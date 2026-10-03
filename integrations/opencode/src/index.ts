import { existsSync } from "node:fs"
import { dirname, isAbsolute, relative, resolve } from "node:path"
import { spawn, spawnSync } from "node:child_process"
import { Plugin } from "@opencode/plugin"

type AnyRecord = Record<string, any>

const ABSOLUTE_PATH = /(?:\/Users\/|\/home\/|\/workspace\/)[^\s"'`]+/g

export default Plugin.define({
  id: "sideeye.opencode",
  async setup(ctx: any) {
    await ctx.command.transform((editor: any) => {
      editor.add({
        name: "sideeye-review",
        description: "Review the current OpenCode session with Side-Eye",
        execute: async ({ sessionID }: { sessionID: string }) => {
          const result = await reviewSession(ctx, sessionID)
          await ctx.session.synthetic({ sessionID, text: result })
        },
      })
    })
  },
})

async function reviewSession(ctx: any, sessionID: string): Promise<string> {
  try {
    const session = await ctx.session.get({ sessionID })
    const client = ctx.client as any
    const response = await client.session.export({
      sessionID,
      sanitize: true,
    })
    const data = response?.data ?? response
    const repo = findRepository(session, data)
    const diffBase = gitMergeBase(repo)
    const touchedFiles = gitTouchedFiles(repo, diffBase, data)
    const wrapper = {
      repo,
      diff_base: diffBase,
      touched_files: touchedFiles,
      data,
    }
    const command = Array.isArray(ctx.options?.command)
      ? ctx.options.command
      : [ctx.options?.command ?? "sideeye"]
    const out = await run(command[0], [
      ...command.slice(1),
      "review",
      "--opencode-export",
      "-",
      "--yes",
      "--json",
      "--repo",
      repo,
      "--diff-base",
      diffBase,
    ], JSON.stringify(wrapper), repo)
    if (out.code !== 0) {
      return `Side-Eye review failed before producing a verdict.\n\n${out.stderr || out.stdout}`
    }
    const record = lastJsonLine(out.stdout)
    if (!record) {
      return `Side-Eye returned no structured verdict.\n\n${out.stdout}`
    }
    // This is the persisted structured judge record, inserted as a synthetic
    // message. It is never sent back through the current model for paraphrase.
    return `Side-Eye verdict (raw structured record):\n${JSON.stringify(record, null, 2)}`
  } catch (error) {
    return `Side-Eye review could not start: ${error instanceof Error ? error.message : String(error)}`
  }
}

function findRepository(session: AnyRecord, exportData: AnyRecord): string {
  const candidates = new Set<string>()
  const sessionDirectory = session?.location?.directory
  if (typeof sessionDirectory === "string") candidates.add(sessionDirectory)
  collectPaths(exportData, candidates)
  for (const candidate of candidates) {
    const root = gitRoot(candidate)
    if (root) return root
  }
  throw new Error("no git repository found from the OpenCode session location or tool paths")
}

function collectPaths(value: unknown, output: Set<string>, depth = 0): void {
  if (depth > 8) return
  if (typeof value === "string") {
    if (isAbsolute(value) && value.length < 4096) output.add(value)
    for (const match of value.match(ABSOLUTE_PATH) ?? []) {
      output.add(match.replace(/[),.;]+$/, ""))
    }
    return
  }
  if (Array.isArray(value)) {
    for (const item of value) collectPaths(item, output, depth + 1)
    return
  }
  if (value && typeof value === "object") {
    for (const item of Object.values(value as AnyRecord)) collectPaths(item, output, depth + 1)
  }
}

function gitRoot(candidate: string): string | undefined {
  const path = existsSync(candidate) ? candidate : dirname(candidate)
  const result = spawnSync("git", ["-C", path, "rev-parse", "--show-toplevel"], {
    encoding: "utf8",
  })
  if (result.status !== 0) return undefined
  const root = result.stdout.trim()
  return root || undefined
}

function gitMergeBase(repo: string): string {
  for (const ref of ["origin/main", "main"]) {
    const result = spawnSync("git", ["-C", repo, "merge-base", "HEAD", ref], {
      encoding: "utf8",
    })
    if (result.status === 0 && result.stdout.trim()) return result.stdout.trim()
  }
  return "HEAD"
}

function gitLines(repo: string, args: string[]): string[] {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) return []
  return result.stdout.split("\n").map((line: string) => line.trim()).filter(Boolean)
}

function gitTouchedFiles(repo: string, diffBase: string, exportData: AnyRecord): AnyRecord[] {
  const files = new Set<string>([
    ...gitLines(repo, ["diff", "--name-only", diffBase]),
    ...gitLines(repo, ["diff", "--name-only"]),
  ])
  const sessionPaths = new Set<string>()
  collectPaths(exportData, sessionPaths)
  for (const entry of gitLines(repo, ["status", "--porcelain=v1"])) {
    const path = entry.slice(3).trim()
    if (!path || !sessionPaths.has(resolve(repo, path))) continue
    files.add(path)
  }
  return [...files].sort().map((path) => ({ path, count: 1 }))
}

function lastJsonLine(output: string): AnyRecord | undefined {
  for (const line of output.trim().split("\n").reverse()) {
    try {
      const value = JSON.parse(line)
      if (value && typeof value === "object" && "score" in value) return value
    } catch {
      // Earlier lines are human progress output; only the final structured line matters.
    }
  }
  return undefined
}

function run(command: string, args: string[], input: string, cwd: string): Promise<{
  code: number
  stdout: string
  stderr: string
}> {
  return new Promise((resolveResult, reject) => {
    const child = spawn(command, args, { cwd, env: process.env })
    let stdout = ""
    let stderr = ""
    child.stdout.on("data", (chunk: Buffer) => { stdout += chunk.toString() })
    child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString() })
    child.on("error", reject)
    child.on("close", (code: number | null) => resolveResult({ code: code ?? 1, stdout, stderr }))
    child.stdin.end(input)
  })
}
