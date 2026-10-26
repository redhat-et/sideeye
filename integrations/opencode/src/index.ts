import { existsSync, statSync } from "node:fs"
import { dirname, isAbsolute, join, resolve } from "node:path"
import { homedir } from "node:os"
import { spawn } from "node:child_process"

type AnyRecord = Record<string, any>
export type ReviewScope = "last" | "all"

const ABSOLUTE_PATH = /(?:\/(?:Users|home|workspace|private|tmp|var)\/)[^\s"'`]+/g

// OpenCode requires a server entrypoint for this package. The user-facing
// command lives in the TUI, so the server plugin intentionally adds nothing.
export default {
  id: "sideeye.opencode",
  setup() {},
}

/** Run the bridge against a full sanitized OpenCode export. */
export async function reviewExport(
  session: AnyRecord,
  data: AnyRecord,
  options: AnyRecord = {},
  commandText = "",
  estimateHint = true,
): Promise<string> {
  try {
    const repo = await findRepository(session, data, options.repo as string | undefined)
    const diffBase = await gitMergeBase(repo)
    const touchedFiles = await gitTouchedFiles(repo, diffBase, data)
    const { confirm, scope, instructions } = parseCommand(commandText)
    if (options.engine === "rust") {
      return reviewWithRust(
        repo,
        diffBase,
        touchedFiles,
        data,
        options,
        confirm,
        instructions,
        scope,
        estimateHint,
      )
    }
    if (scope === "last") {
      return "Last-exchange scope requires the Rust engine (`engine: rust`). Re-run setup with `sideeye` on PATH."
    }
    if (instructions) {
      return "Focus instructions require the Rust engine (`engine: rust`). Re-run setup with `sideeye` on PATH."
    }
    const wrapper = { repo, diff_base: diffBase, touched_files: touchedFiles, data }
    const command = Array.isArray(options.command) ? options.command : [options.command ?? "sideeye-py"]
    const estimate = await run(command[0], [
      ...command.slice(1), "review", "--opencode-export", "-", "--estimate-only", "--json",
      "--repo", repo, "--diff-base", diffBase,
    ], JSON.stringify(wrapper), repo)
    if (estimate.code !== 0) {
      return `Side-Eye estimate failed before producing a verdict.\n\n${estimate.stderr || estimate.stdout}`
    }
    const estimateRecord = lastJsonLine(estimate.stdout)
    if (!estimateRecord) return `Side-Eye returned no structured estimate.\n\n${estimate.stdout}`
    if (!confirm) return formatEstimate(estimateRecord)

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

async function reviewWithRust(
  repo: string,
  diffBase: string,
  touchedFiles: AnyRecord[],
  data: AnyRecord,
  options: AnyRecord,
  confirm: boolean,
  instructions: string,
  scope: ReviewScope | undefined,
  estimateHint: boolean,
): Promise<string> {
  const rust = Array.isArray(options.rust) ? options.rust : [options.rust ?? "sideeye"]
  const model = typeof options.model === "string" ? options.model : "claude-opus-4-8"
  const maxTokens = Number.isInteger(options.maxTokens) ? String(options.maxTokens) : "4096"
  const args = [...rust.slice(1)]
  const focus = instructions ? ["--instructions", instructions] : []
  const timeoutMs = Number.isInteger(options.timeoutMs) ? Number(options.timeoutMs) : 300000
  const outPath =
    typeof options.out === "string"
      ? options.out
      : join(homedir(), ".local", "share", "sideeye", "opencode-verdicts.jsonl")
  const touched = touchedFiles.flatMap((file) => ["--touched-file", String(file.path)])
  const packetScope = scope === "last" ? "last-exchange" : "session"
  const packetRun = await run(rust[0], [
    ...args, "packet", "--opencode-export", "-", "--redact",
    "--repo", repo, "--diff-base", diffBase, "--scope", packetScope, ...touched,
  ], JSON.stringify(data), repo, timeoutMs)
  if (packetRun.code !== 0) {
    return `Side-Eye packet build failed before producing a verdict.\n\n${packetRun.stderr || packetRun.stdout}`
  }
  const packet = lastJsonObject(packetRun.stdout)
  if (!packet) return `Side-Eye returned no packet.\n\n${packetRun.stdout}`
  const packetJson = JSON.stringify(packet)

  const estimate = await run(rust[0], [
    ...args, "estimate", "-", "--model", model, "--max-tokens", maxTokens, ...focus,
  ], packetJson, repo, timeoutMs)
  if (estimate.code !== 0) {
    return `Side-Eye estimate failed before producing a verdict.\n\n${estimate.stderr || estimate.stdout}`
  }
  const estimateRecord = lastJsonObject(estimate.stdout)
  if (!estimateRecord) return `Side-Eye returned no structured estimate.\n\n${estimate.stdout}`
  if (!confirm) return formatEstimate(estimateRecord, instructions, scope, estimateHint)

  const out = await run(rust[0], [
    ...args, "review", "-", "--provider", "anthropic",
    "--model", model, "--max-tokens", maxTokens, ...focus,
    "--out", outPath,
  ], packetJson, repo, timeoutMs)
  if (out.code !== 0) {
    return `Side-Eye review failed before producing a verdict.\n\n${out.stderr || out.stdout}`
  }
  const record = lastJsonLine(out.stdout)
  if (!record) return `Side-Eye returned no structured verdict.\n\n${out.stdout}`
  return `Side-Eye verdict (raw structured record):\n${JSON.stringify(record, null, 2)}`
}

export function formatEstimate(
  estimate: AnyRecord,
  instructions = "",
  scope?: ReviewScope,
  showHint = true,
): string {
  const scopeLabel = scope === "last" ? "last exchange" : "whole session"
  const scopeToken = scope === "last" ? "last " : scope === "all" ? "all " : ""
  const hint = `Run /sideeye ${scopeToken}--confirm${instructions ? ` ${instructions}` : ""} to submit this packet to the judge.`
  return [
    "Side-Eye estimate (no judge call made):",
    `  estimated cost: $${Number(estimate.estimated_cost_usd ?? 0).toFixed(4)}`,
    `  input tokens: ${estimate.input_tokens ?? "unknown"}${estimate.exact === false ? " (estimated)" : ""}`,
    `  code: ${estimate.adapter_version ?? (estimate.max_tokens ? `rust (max ${estimate.max_tokens} output tokens)` : "unknown")}`,
    ...(typeof estimate.model === "string" ? [`  judge: ${estimate.model}`] : []),
    ...(typeof estimate.turns === "number"
      ? [`  scope: ${scopeLabel} · ${estimate.turns} turns · ${estimate.has_diff ? `git diff (${estimate.artifacts} artifact(s), ${estimate.diff_complete ? "complete" : "partial"})` : "narrative only"}`]
      : []),
    ...(instructions ? [`  request: ${instructions}`] : []),
    ...(showHint ? ["", hint] : []),
  ].join("\n")
}

export function parseCommand(commandText: string): {
  confirm: boolean
  scope?: ReviewScope
  instructions: string
} {
  const confirm = /\B--confirm\b/.test(commandText)
  let rest = commandText.replace(/\B--confirm\b/g, "").trim()
  let scope: ReviewScope | undefined
  const match = rest.match(/^(last|last-exchange|last_exchange|all|session|context)(?:\s+|$)/i)
  if (match) {
    scope = match[1].toLowerCase().startsWith("last") ? "last" : "all"
    rest = rest.slice(match[0].length).trim()
  }
  return {confirm, scope, instructions: rest}
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
  for (const candidate of candidates) {
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
  let path: string
  try {
    path = existsSync(candidate) && statSync(candidate).isDirectory() ? candidate : dirname(candidate)
  } catch {
    return undefined
  }
  if (!existsSync(path)) return undefined
  const result = await runProcess("git", ["-C", path, "rev-parse", "--show-toplevel"], path)
  if (result.status !== 0) return undefined
  const root = result.stdout.trim()
  return root || undefined
}

async function gitMergeBase(repo: string): Promise<string> {
  // Review against the project base, not the branch's own upstream. On a
  // feature branch whose upstream is its own tip, using upstream makes
  // merge-base equal HEAD and silently produces an empty code diff.
  const refs = ["origin/main", "main", "master"]
  for (const ref of refs) {
    const result = await runProcess("git", ["-C", repo, "merge-base", "HEAD", ref], repo)
    if (result.status === 0 && result.stdout.trim()) return result.stdout.trim()
  }
  const upstream = await runProcess(
    "git",
    ["-C", repo, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    repo,
  )
  if (upstream.status === 0 && upstream.stdout.trim()) {
    const ref = upstream.stdout.trim()
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

function lastJsonObject(output: string): AnyRecord | undefined {
  for (const line of output.trim().split("\n").reverse()) {
    try {
      const value = JSON.parse(line)
      if (value && typeof value === "object" && !Array.isArray(value)) return value
    } catch {
      // Progress lines are not JSON.
    }
  }
  return undefined
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

function run(command: string, args: string[], input: string, cwd: string, timeoutMs = 300000): Promise<{
  code: number
  stdout: string
  stderr: string
}> {
  return runProcess(command, args, cwd, input, timeoutMs).then((result) => ({
    code: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  }))
}

function runProcess(
  command: string,
  args: string[],
  cwd: string,
  input = "",
  timeoutMs = 300000,
): Promise<{
  status: number
  stdout: string
  stderr: string
}> {
  return new Promise((resolveResult, reject) => {
    const child = spawn(command, args, { cwd, env: process.env })
    let stdout = ""
    let stderr = ""
    let settled = false
    const finish = (status: number, extra = "") => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolveResult({ status, stdout, stderr: stderr + extra })
    }
    const timer = setTimeout(() => {
      if (settled) return
      child.kill("SIGKILL")
      finish(1, `\nsideeye: timed out after ${timeoutMs}ms`)
    }, timeoutMs)
    child.stdout.on("data", (chunk: Buffer) => { stdout += chunk.toString() })
    child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString() })
    child.on("error", reject)
    child.stdin.on("error", (error: NodeJS.ErrnoException) => {
      // The bridge can reject before consuming stdin (for example when the
      // judge route is missing). EPIPE is then expected and must not obscure
      // the actionable Side-Eye error.
      if (error.code !== "EPIPE") stderr += `\n${error.message}`
    })
    child.on("close", (code: number | null) => finish(code ?? 1))
    if (input) child.stdin.end(input)
    else child.stdin.end()
  })
}

// Kept as a narrow test seam; the plugin's public surface remains the
// OpenCode Plugin definition above.
export const testHelpers = {
  findRepository,
  formatEstimate,
  gitMergeBase,
  gitTouchedFiles,
  parseCommand,
  reviewExport,
  run,
}
