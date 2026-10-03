import { existsSync, statSync } from "node:fs"
import { dirname, isAbsolute, resolve } from "node:path"
import { spawn } from "node:child_process"
import { Plugin } from "@opencode/plugin"
import type { CommandInvocation } from "@opencode/plugin/promise/command"
import type { Context as ServerContext } from "@opencode/plugin/promise/plugin"

type AnyRecord = Record<string, any>

const ABSOLUTE_PATH = /(?:\/(?:Users|home|workspace|private|tmp|var)\/)[^\s"'`]+/g

export default Plugin.define({
  id: "sideeye.opencode",
  async setup(ctx: ServerContext) {
    await ctx.command.transform((editor) => {
      editor.add({
        name: "sideeye-review",
        description: "Review the current OpenCode session with Side-Eye",
        execute: async ({ sessionID, prompt }: CommandInvocation) => {
          const result = await reviewSession(ctx, sessionID, prompt.text)
          await ctx.session.synthetic({ sessionID, text: result })
        },
      })
    })
  },
})

async function reviewSession(ctx: ServerContext, sessionID: string, commandText = ""): Promise<string> {
  try {
    const session = await ctx.session.get({ sessionID })
    const context = await ctx.session.context({ sessionID })
    const data = contextToExport(sessionID, context)
    return reviewExport(session, data, ctx.options, commandText)
  } catch (error) {
    return `Side-Eye review could not start: ${error instanceof Error ? error.message : String(error)}`
  }
}

/** Run the bridge against either server-projected context or a full client export. */
export async function reviewExport(
  session: AnyRecord,
  data: AnyRecord,
  options: AnyRecord = {},
  commandText = "",
): Promise<string> {
  try {
    const repo = await findRepository(session, data, options.repo as string | undefined)
    const diffBase = await gitMergeBase(repo)
    const touchedFiles = await gitTouchedFiles(repo, diffBase, data)
    const wrapper = { repo, diff_base: diffBase, touched_files: touchedFiles, data }
    const command = Array.isArray(options.command) ? options.command : [options.command ?? "sideeye"]
    const estimate = await run(command[0], [
      ...command.slice(1), "review", "--opencode-export", "-", "--estimate-only", "--json",
      "--repo", repo, "--diff-base", diffBase,
    ], JSON.stringify(wrapper), repo)
    if (estimate.code !== 0) {
      return `Side-Eye estimate failed before producing a verdict.\n\n${estimate.stderr || estimate.stdout}`
    }
    const estimateRecord = lastJsonLine(estimate.stdout)
    if (!estimateRecord) return `Side-Eye returned no structured estimate.\n\n${estimate.stdout}`
    if (!/\B--confirm\b/.test(commandText)) return formatEstimate(estimateRecord)

    const out = await run(command[0], [
      ...command.slice(1), "review", "--opencode-export", "-", "--yes", "--json",
      "--repo", repo, "--diff-base", diffBase,
    ], JSON.stringify(wrapper), repo)
    if (out.code !== 0) {
      return `Side-Eye review failed before producing a verdict.\n\n${out.stderr || out.stdout}`
    }
    const record = lastJsonLine(out.stdout)
    if (!record) return `Side-Eye returned no structured verdict.\n\n${out.stdout}`
    return `Side-Eye verdict (raw structured record):\n${JSON.stringify(record, null, 2)}`
  } catch (error) {
    return `Side-Eye review could not start: ${error instanceof Error ? error.message : String(error)}`
  }
}

function contextToExport(sessionID: string, context: AnyRecord): AnyRecord {
  const model = context.model ?? {}
  return {
    info: {
      id: sessionID,
      model: { providerID: model.providerID, id: model.id },
    },
    messages: (context.messages ?? []).flatMap((message: AnyRecord) => normalizeMessage(message, model)),
  }
}

function normalizeMessage(message: AnyRecord, model: AnyRecord): AnyRecord[] {
  if (message.role === "user") {
    return [{ type: "user", text: textParts(message.content) }]
  }
  if (message.role === "assistant") {
    return [{
      type: "assistant",
      model: { providerID: model.providerID, id: model.id },
      content: (message.content ?? []).flatMap((part: AnyRecord) => normalizePart(part)),
    }]
  }
  if (message.role === "tool") {
    return [{
      type: "assistant",
      model: { providerID: model.providerID, id: model.id },
      content: (message.content ?? []).map((part: AnyRecord) => ({
        type: "tool",
        name: part.name ?? "tool",
        state: {
          status: part.type === "tool-result" && part.result?.type === "error" ? "error" : "completed",
          input: part.input,
          error: part.result?.type === "error" ? part.result.value : undefined,
          content: [{ type: "text", text: resultText(part.result) }],
        },
      })),
    }]
  }
  return []
}

function normalizePart(part: AnyRecord): AnyRecord[] {
  if (part.type === "text") return [part]
  if (part.type === "tool-call") {
    return [{ type: "tool", name: part.name, state: { status: "running", input: part.input } }]
  }
  if (part.type === "tool-result") {
    return [{
      type: "tool",
      name: part.name,
      state: {
        status: part.result?.type === "error" ? "error" : "completed",
        input: part.input,
        error: part.result?.type === "error" ? part.result.value : undefined,
        content: [{ type: "text", text: resultText(part.result) }],
      },
    }]
  }
  return []
}

function textParts(parts: unknown): string {
  return Array.isArray(parts)
    ? parts.filter((part): part is AnyRecord => Boolean(part && typeof part === "object" && part.type === "text"))
      .map((part) => part.text).join("")
    : ""
}

function resultText(result: AnyRecord | undefined): string {
  if (!result) return "(no output)"
  if (result.type === "text" || result.type === "error") return String(result.value)
  if (result.type === "json") return JSON.stringify(result.value)
  if (result.type === "content") return textParts(result.value)
  return JSON.stringify(result)
}

function formatEstimate(estimate: AnyRecord): string {
  return [
    "Side-Eye estimate (no judge call made):",
    `  estimated cost: $${Number(estimate.estimated_cost_usd ?? 0).toFixed(4)}`,
    `  input tokens: ${estimate.input_tokens ?? "unknown"}${estimate.exact ? "" : " (estimated)"}`,
    `  code: ${estimate.adapter_version ?? "unknown"}`,
    "",
    "Run /sideeye --confirm to submit this packet to the judge.",
  ].join("\n")
}

async function findRepository(session: AnyRecord, exportData: AnyRecord, configured?: string): Promise<string> {
  if (configured) {
    const root = await gitRoot(configured)
    if (!root) throw new Error(`configured repository is not a git checkout: ${configured}`)
    return root
  }
  const candidates = new Set<string>()
  const sessionDirectory = session?.location?.directory
  if (typeof sessionDirectory === "string") candidates.add(sessionDirectory)
  collectInputPaths(exportData, candidates)
  const roots = new Map<string, number>()
  for (const candidate of [...candidates].slice(0, 64)) {
    const root = await gitRoot(candidate)
    if (root) roots.set(root, (roots.get(root) ?? 0) + 1)
  }
  const ranked = [...roots.entries()].sort((left, right) => right[1] - left[1])
  if (!ranked.length) throw new Error("no git repository found from the OpenCode session location or tool inputs")
  if (ranked.length > 1 && ranked[0][1] === ranked[1][1]) {
    throw new Error(`multiple git repositories found; configure the plugin repo option (${ranked.map(([root]) => root).join(", ")})`)
  }
  return ranked[0][0]
}

function collectInputPaths(exportData: AnyRecord, output: Set<string>): void {
  for (const message of exportData.messages ?? []) {
    if (message.type === "user") collectPaths(message.files, output)
    for (const part of message.content ?? []) {
      if (part.type === "tool") collectPaths(part.state?.input, output)
    }
  }
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

async function gitRoot(candidate: string): Promise<string | undefined> {
  const path = existsSync(candidate) && statSync(candidate).isDirectory() ? candidate : dirname(candidate)
  const result = await runProcess("git", ["-C", path, "rev-parse", "--show-toplevel"], path)
  if (result.status !== 0) return undefined
  const root = result.stdout.trim()
  return root || undefined
}

async function gitMergeBase(repo: string): Promise<string> {
  const upstream = await runProcess(
    "git",
    ["-C", repo, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    repo,
  )
  const refs = [upstream.status === 0 ? upstream.stdout.trim() : "", "origin/main", "main", "master"]
  for (const ref of refs.filter(Boolean)) {
    const result = await runProcess("git", ["-C", repo, "merge-base", "HEAD", ref], repo)
    if (result.status === 0 && result.stdout.trim()) return result.stdout.trim()
  }
  return "HEAD"
}

async function gitLines(repo: string, args: string[]): Promise<string[]> {
  const result = await runProcess("git", ["-C", repo, ...args], repo)
  if (result.status !== 0) return []
  return result.stdout.split("\n").map((line: string) => line.trim()).filter(Boolean)
}

async function gitTouchedFiles(repo: string, diffBase: string, exportData: AnyRecord): Promise<AnyRecord[]> {
  const files = new Set<string>([
    ...(await gitLines(repo, ["diff", "--name-only", diffBase])),
    ...(await gitLines(repo, ["diff", "--name-only"])),
  ])
  const sessionPaths = new Set<string>()
  collectInputPaths(exportData, sessionPaths)
  for (const entry of await gitLines(repo, ["status", "--porcelain=v1"])) {
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
      if (value && typeof value === "object" && ("score" in value || value.type === "estimate")) return value
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
  return runProcess(command, args, cwd, input).then((result) => ({
    code: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  }))
}

function runProcess(command: string, args: string[], cwd: string, input = ""): Promise<{
  status: number
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
    child.stdin.on("error", (error: Error) => { stderr += `\n${error.message}` })
    child.on("close", (code: number | null) => resolveResult({ status: code ?? 1, stdout, stderr }))
    if (input) child.stdin.end(input)
    else child.stdin.end()
  })
}

// Kept as a narrow test seam; the plugin's public surface remains the
// OpenCode Plugin definition above.
export const testHelpers = {
  contextToExport,
  findRepository,
  gitMergeBase,
  gitTouchedFiles,
  reviewExport,
  run,
}
