import { execFileSync } from "node:child_process"
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync as writeFile } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { test } from "node:test"
import assert from "node:assert/strict"
import { testHelpers } from "../src/index.js"
import { pickAndReview } from "../src/tui.js"

function git(cwd: string, args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" }).trim()
}

test("resolves a nested repository from tool input and preserves changed files", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-workspace-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "before\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "initial"])
  writeFile(file, "after\n")

  const exportData = {
    messages: [{
      type: "assistant",
      content: [{type: "tool", state: {input: {path: file}}}],
    }],
  }
  const resolved = await testHelpers.findRepository(
    {location: {directory: workspace}},
    exportData,
  )
  assert.equal(resolved, git(repo, ["rev-parse", "--show-toplevel"]))
  assert.deepEqual(await testHelpers.gitTouchedFiles(repo, "HEAD", exportData), [
    {path: "answer.txt", count: 1},
  ])
})

test("diff base prefers origin/main over a self-tracking upstream", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-base-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "base\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "base"])
  const base = git(repo, ["rev-parse", "HEAD"])
  // Simulate origin/main at the base commit, then advance the feature branch.
  git(repo, ["update-ref", "refs/remotes/origin/main", base])
  git(repo, ["checkout", "-qb", "feature"])
  writeFile(file, "feature\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "feature"])
  // A self-tracking upstream would make merge-base equal HEAD.
  const feature = git(repo, ["rev-parse", "HEAD"])
  git(repo, ["update-ref", "refs/remotes/origin/feature", feature])
  git(repo, ["config", "branch.feature.remote", "origin"])
  git(repo, ["config", "branch.feature.merge", "refs/heads/feature"])

  assert.equal(await testHelpers.gitMergeBase(repo), base)
  const touched = await testHelpers.gitTouchedFiles(repo, await testHelpers.gitMergeBase(repo), {
    messages: [{type: "assistant", content: [{type: "tool", state: {input: {path: file}}}]}],
  })
  assert.deepEqual(touched, [{path: "answer.txt", count: 1}])
})

test("runs a configured engine without a shell and tolerates early stdin close", async () => {
  const directory = mkdtempSync(join(tmpdir(), "sideeye-opencode-engine-"))
  const script = join(directory, "fake-engine.mjs")
  writeFile(script, "process.stdout.write(JSON.stringify({type:'estimate', estimated_cost_usd:0}) + '\\n'); process.exit(0)\n")
  const result = await testHelpers.run(process.execPath, [script], "x".repeat(1_000_000), directory)
  assert.equal(result.code, 0)
  assert.match(result.stdout, /\"type\":\"estimate\"/)
})

test("parses review scope, instructions, and confirmation", () => {
  assert.deepEqual(testHelpers.parseCommand("last security --confirm"), {
    confirm: true,
    scope: "last",
    instructions: "security",
  })
  assert.deepEqual(testHelpers.parseCommand("all missing tests"), {
    confirm: false,
    scope: "all",
    instructions: "missing tests",
  })
  assert.deepEqual(testHelpers.parseCommand("fix the parser"), {
    confirm: false,
    scope: undefined,
    instructions: "fix the parser",
  })
  assert.deepEqual(testHelpers.parseCommand("--confirm"), {
    confirm: true,
    scope: undefined,
    instructions: "",
  })
})

test("estimate shows the scope and the matching confirmation command", () => {
  const estimate = {
    estimated_cost_usd: 0.12,
    input_tokens: 12,
    model: "claude-opus-4-8",
    max_tokens: 2048,
    turns: 2,
    artifacts: 1,
    has_diff: true,
    diff_complete: true,
  }
  const last = testHelpers.formatEstimate(estimate, "security", "last")
  assert.match(last, /scope: last exchange · 2 turns/)
  assert.match(last, /Run \/sideeye last --confirm security/)
  const all = testHelpers.formatEstimate(estimate, "", "all")
  assert.match(all, /scope: whole session · 2 turns/)
  assert.match(all, /Run \/sideeye all --confirm/)
  const unknown = testHelpers.formatEstimate({...estimate, cost_known: false}, "", "all")
  assert.match(unknown, /estimated cost: unknown \(provider pricing not configured\)/)
})

test("non-rust engine fails closed without a Python fallback", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-no-rust-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  const result = await testHelpers.reviewExport(
    {location: {directory: repo}},
    {messages: [{type: "user", text: "task"}]},
    {command: ["python-engine"], repo},
  )
  assert.match(result, /requires the Rust engine/i)
})

test("rust engine builds a packet, estimates, then reviews on confirm", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-rust-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "before\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "initial"])
  writeFile(file, "after\n")

  const engine = join(workspace, "fake-rust.mjs")
  writeFile(engine, [
    "let input='';",
    "process.stdin.on('data', (chunk) => input += chunk);",
    "process.stdin.on('end', () => {",
    "  const command = process.argv[2];",
    "  const payload = command === 'packet' ? {",
    "    schema_version: 'sideeye.review_packet.v1', packet_id: 'p', session_id: 's',",
    "    source: 'opencode', client: 'opencode', generator_model: 'g', task: 'task',",
    "    turns: [{role: 'user', text: 'task'}], artifacts: [], generation_usage: null",
    "  } : command === 'estimate' ? {",
    "    input_tokens: 10, estimated_cost_usd: 0.01, model: 'claude-opus-4-8', max_tokens: 2048, turns: 2, artifacts: 1, has_diff: true, diff_complete: true",
    "  } : { score: 5, correctness: 'correct', claims_supported: true, summary: 'rust fixture verdict ' + process.argv.slice(2).join(' ') };",
    "  process.stdout.write(JSON.stringify(payload) + '\\n');",
    "});",
  ].join("\n"))

  const session = {location: {directory: repo}}
  const data = {
    info: {id: "ses-rust", model: {providerID: "openai", id: "gpt-test"}},
    messages: [
      {type: "user", text: "task"},
      {type: "assistant", model: {providerID: "openai", id: "gpt-test"}, content: [
        {type: "tool", name: "read", state: {status: "completed", input: {path: file}, content: [{type: "text", text: "answer"}]}},
      ]},
    ],
  }
  const options = {engine: "rust", rust: [process.execPath, engine], provider: "openai-compatible"}

  const estimate = await testHelpers.reviewExport(session, data, options)
  assert.match(estimate, /estimate \(no judge call made\)/i)
  assert.match(estimate, /output cap: 2048 tokens/)
  assert.match(estimate, /judge: claude-opus-4-8/)
  assert.match(estimate, /scope: whole session · 2 turns · git diff \(1 artifact\(s\), complete\)/)

  const verdict = await testHelpers.reviewExport(session, data, options, "focus on security --confirm")
  assert.match(verdict, /rust fixture verdict/)
  assert.match(verdict, /--provider openai-compatible/)
  assert.match(verdict, /--instructions focus on security/)
  assert.match(verdict, /--out /)
})

test("last scope is passed to the rust packet builder", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-scope-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "base\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "base"])
  writeFile(file, "after\n")
  const argvLog = join(workspace, "argv.log")
  const engine = join(workspace, "fake-rust.mjs")
  writeFile(engine, [
    "import {appendFileSync} from 'node:fs';",
    `const log = ${JSON.stringify(argvLog)};`,
    "appendFileSync(log, process.argv.slice(2).join(' ') + '\\n');",
    "let input='';",
    "process.stdin.on('data', (chunk) => input += chunk);",
    "process.stdin.on('end', () => {",
    "  const command = process.argv[2];",
    "  const payload = command === 'packet' ? {ok: true} : command === 'estimate' ? {input_tokens: 1, estimated_cost_usd: 0, turns: 1, artifacts: 1, has_diff: true, diff_complete: true} : {score: 5, summary: 'scope fixture verdict'};",
    "  process.stdout.write(JSON.stringify(payload) + '\\n');",
    "});",
  ].join("\n"))
  const session = {location: {directory: repo}}
  const data = {
    info: {id: "ses-scope", model: {providerID: "openai", id: "gpt-test"}},
    messages: [
      {type: "user", text: "first"},
      {type: "assistant", model: {providerID: "openai", id: "gpt-test"}, content: [{type: "text", text: "first answer"}]},
      {type: "user", text: "second"},
      {type: "assistant", model: {providerID: "openai", id: "gpt-test"}, content: [{type: "text", text: "second answer"}]},
    ],
  }
  const verdict = await testHelpers.reviewExport(
    session,
    data,
    {engine: "rust", rust: [process.execPath, engine]},
    "last --confirm",
  )
  assert.match(verdict, /scope fixture verdict/)
  const packetLine = readFileSync(argvLog, "utf8")
    .split("\n")
    .find((line) => line.startsWith("packet "))
  assert.ok(packetLine, "packet command was invoked")
  assert.match(packetLine, /--scope last-exchange/)
})

test("picker selects last exchange, collects instructions, and confirms", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-picker-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "base\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "base"])
  writeFile(file, "after\n")
  const argvLog = join(workspace, "argv.log")
  const engine = join(workspace, "fake-rust.mjs")
  writeFile(engine, [
    "import {appendFileSync} from 'node:fs';",
    `const log = ${JSON.stringify(argvLog)};`,
    "appendFileSync(log, process.argv.slice(2).join(' ') + '\\n');",
    "let input='';",
    "process.stdin.on('data', (chunk) => input += chunk);",
    "process.stdin.on('end', () => {",
    "  const command = process.argv[2];",
    "  const payload = command === 'packet' ? {ok: true} : command === 'estimate' ? {input_tokens: 1, estimated_cost_usd: 0, model: 'claude-opus-4-8', max_tokens: 4096, turns: 2, artifacts: 1, has_diff: true, diff_complete: true} : {score: 5, summary: 'picker verdict'};",
    "  process.stdout.write(JSON.stringify(payload) + '\\n');",
    "});",
  ].join("\n"))
  const data = {
    info: {id: "ses-picker", model: {providerID: "openai", id: "gpt-test"}},
    messages: [
      {type: "user", text: "first"},
      {type: "assistant", model: {providerID: "openai", id: "gpt-test"}, content: [{type: "text", text: "first answer"}]},
      {type: "user", text: "second"},
      {type: "assistant", model: {providerID: "openai", id: "gpt-test"}, content: [{type: "text", text: "second answer"}]},
    ],
  }
  const synthetic: string[] = []
  const toasts: string[] = []
  let confirmation = ""
  let exportOptions: Record<string, any> | undefined
  const context = {
    options: {engine: "rust", rust: [process.execPath, engine], repo},
    client: {
      session: {
        get: async () => ({location: {directory: repo}}),
        export: async (options: Record<string, any>) => { exportOptions = options; return data },
        synthetic: async ({text}: {text: string}) => { synthetic.push(text) },
      },
    },
    ui: {
      dialog: {
        select: async () => "last",
        prompt: async () => "focus on tests",
        confirm: async (options: {message: string}) => {
          confirmation = options.message
          return true
        },
        alert: async ({message}: {message: string}) => { throw new Error(`unexpected alert: ${message}`) },
      },
      toast: {show: (options: {message: string}) => { toasts.push(options.message) }},
    },
  }

  await pickAndReview(context as any, "ses-picker")

  assert.equal(synthetic.length, 1)
  assert.equal(exportOptions?.sanitize, false)
  assert.ok(toasts.some((message) => message.includes("Preparing your estimate")))
  assert.ok(toasts.some((message) => message.includes("The judge is reviewing")))
  assert.match(synthetic[0], /picker verdict/)
  assert.match(confirmation, /Side-Eye estimate/)
  assert.match(confirmation, /scope: last exchange/)
  const lines = readFileSync(argvLog, "utf8").split("\n")
  const packetLine = lines.find((line) => line.startsWith("packet "))
  assert.ok(packetLine, "packet command was invoked")
  assert.match(packetLine, /--scope last-exchange/)
  const estimateLine = lines.find((line) => line.startsWith("estimate "))
  assert.ok(estimateLine, "estimate command was invoked")
  assert.match(estimateLine, /--instructions focus on tests/)
})

test("context overflow retries with a smaller packet budget", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-fit-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "base\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "base"])
  writeFile(file, "after\n")
  const engine = join(workspace, "fake-rust.mjs")
  writeFile(engine, [
    "let input='';",
    "process.stdin.on('data', (chunk) => input += chunk);",
    "process.stdin.on('end', () => {",
    "  const args = process.argv.slice(2);",
    "  const command = args[0];",
    "  if (command === 'packet') {",
    "    const index = args.indexOf('--max-packet-chars');",
    "    const max = Number(args[index + 1]);",
    "    process.stdout.write(JSON.stringify({large: max >= 300000}) + '\\n');",
    "    return;",
    "  }",
    "  if (command === 'estimate') {",
    "    const packet = JSON.parse(input);",
    "    if (packet.large) {",
    "      console.error(\"sideeye: provider configuration error: packet is 217138 input tokens; claude-opus-4-8's context window is 200000 (need 221234 with output).\");",
    "      process.exit(1);",
    "    }",
    "    process.stdout.write(JSON.stringify({input_tokens: 1000, estimated_cost_usd: 0.01, model: 'm', max_tokens: 4096, turns: 1, artifacts: 1, has_diff: true, diff_complete: true, exact: true, cost_known: true}) + '\\n');",
    "    return;",
    "  }",
    "  process.stdout.write(JSON.stringify({score: 5, summary: 'trimmed verdict'}) + '\\n');",
    "});",
  ].join("\n"))
  const session = {location: {directory: repo}}
  const data = {
    info: {id: "ses-fit", model: {providerID: "openai", id: "gpt-test"}},
    messages: [{type: "user", text: "task"}],
  }
  const options = {engine: "rust", rust: [process.execPath, engine]}
  const estimate = await testHelpers.reviewExport(session, data, options)
  assert.match(estimate, /auto-trimmed to fit the judge context/)
  const verdict = await testHelpers.reviewExport(session, data, options, "--confirm")
  assert.match(verdict, /trimmed verdict/)
})

test("rust engine call times out instead of hanging", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-timeout-"))
  const repo = join(workspace, "project")
  mkdirSync(repo)
  git(repo, ["init", "-q"])
  git(repo, ["config", "user.email", "test@example.invalid"])
  git(repo, ["config", "user.name", "Side-Eye Test"])
  const file = join(repo, "answer.txt")
  writeFile(file, "base\n")
  git(repo, ["add", "answer.txt"])
  git(repo, ["commit", "-qm", "base"])
  const engine = join(workspace, "slow-engine.mjs")
  writeFile(engine, "setTimeout(() => {}, 5000)\n")
  const data = {
    info: {id: "ses-slow", model: {providerID: "openai", id: "gpt-test"}},
    messages: [{type: "user", text: "task"}],
  }
  const result = await testHelpers.reviewExport(
    {location: {directory: repo}},
    data,
    {engine: "rust", rust: [process.execPath, engine], timeoutMs: 100},
  )
  assert.match(result, /timed out after 100ms/i)
})
