import { execFileSync } from "node:child_process"
import { mkdtempSync, mkdirSync, writeFileSync as writeFile } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { test } from "node:test"
import assert from "node:assert/strict"
import { testHelpers } from "../src/index.js"

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

test("runs a configured engine without a shell and tolerates early stdin close", async () => {
  const directory = mkdtempSync(join(tmpdir(), "sideeye-opencode-engine-"))
  const script = join(directory, "fake-engine.mjs")
  writeFile(script, "process.stdout.write(JSON.stringify({type:'estimate', estimated_cost_usd:0}) + '\\n'); process.exit(0)\n")
  const result = await testHelpers.run(process.execPath, [script], "x".repeat(1_000_000), directory)
  assert.equal(result.code, 0)
  assert.match(result.stdout, /\"type\":\"estimate\"/)
})

test("projects server session context into the adapter envelope", () => {
  const projected = testHelpers.contextToExport("ses-test", {
    model: {providerID: "openai", id: "gpt-test"},
    messages: [{role: "user", content: [{type: "text", text: "Fix it"}]}],
  })
  assert.equal(projected.info.id, "ses-test")
  assert.equal(projected.messages[0].type, "user")
  assert.equal(projected.messages[0].text, "Fix it")
})

test("review export estimates first and requires explicit confirmation", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "sideeye-opencode-review-"))
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
  const fake = join(workspace, "fake-engine.mjs")
  writeFile(fake, [
    "const estimate = process.argv.includes('--estimate-only');",
    "process.stdout.write(JSON.stringify(estimate ? {type:'estimate', estimated_cost_usd:0.12, input_tokens:12, exact:true, adapter_version:'v1-sighted'} : {score:5, summary:'fixture verdict'}) + '\\n');",
  ].join("\n"))
  const data = {
    messages: [{type: "assistant", content: [{type: "tool", state: {input: {path: file}}}]}],
  }
  const options = {command: [process.execPath, fake], repo}
  const estimate = await testHelpers.reviewExport({location: {directory: repo}}, data, options)
  assert.match(estimate, /estimate \(no judge call made\)/i)
  const verdict = await testHelpers.reviewExport({location: {directory: repo}}, data, options, "--confirm")
  assert.match(verdict, /fixture verdict/)
})
