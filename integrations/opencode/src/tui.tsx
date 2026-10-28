import { existsSync, readFileSync } from "node:fs"
import type { Context } from "@opencode/plugin/tui/context"
import {
  formatVerdict,
  labelLatestVerdict,
  reviewExport,
  verdictLedger,
  type ReviewScope,
} from "./index.js"

const USAGE = [
  "Side-Eye review",
  "",
  "/sideeye                 type focus text, then choose the scope",
  "/sideeye last [text]     estimate the last request + response",
  "/sideeye all [text]      estimate the whole session",
  "/sideeye --confirm       submit the whole session after the estimate",
  "",
  "Add --confirm to any scope to submit it. Text after the scope is extra",
  "reviewer instruction on top of the rubric.",
].join("\n")

function showProgress(context: Context, title: string, lines: string[]) {
  try {
    const pending = context.ui.dialog.alert({
      title: "Side-Eye",
      message: [title, "", ...lines].join("\n"),
    })
    void pending.catch(() => {})
  } catch {
    context.ui.toast.show({
      title: "Side-Eye",
      message: title,
      variant: "info",
      duration: 15000,
    })
    return {close: () => {}}
  }
  return {
    close() {
      try {
        context.ui.dialog.clear()
      } catch {
        // The host may have closed the dialog already.
      }
    },
  }
}

async function showVerdict(
  context: Context,
  verdict: string,
): Promise<{label: string; ledger: string}> {
  const ledger = verdictLedger(context.options)
  await context.ui.dialog.alert({title: "Side-Eye verdict", message: verdict})
  const label = await context.ui.dialog.select<"confirmed" | "false-positive" | "neutral" | "skip">({
    title: "Label this review",
    placeholder: "Your label is stored locally for judge-success metrics.",
    options: [
      {title: "Confirmed", value: "confirmed", description: "The finding is valid"},
      {title: "False positive", value: "false-positive", description: "Not a real issue"},
      {title: "Neutral", value: "neutral", description: "Unsure"},
      {title: "Skip", value: "skip", description: "Do not record a label"},
    ],
  })
  if (!label || label === "skip") return {label: "not set", ledger}
  try {
    await labelLatestVerdict(context.options, label)
    context.ui.toast.show({
      title: "Side-Eye",
      message: `Label saved: ${label}`,
      variant: "success",
    })
    return {label, ledger}
  } catch (error) {
    await context.ui.dialog.alert({
      title: "Side-Eye label failed",
      message: error instanceof Error ? error.message : String(error),
    })
    return {label: "save failed", ledger}
  }
}

function reviewCompletion(
  verdict: string,
  label: string,
  ledger: string,
): string {
  const scoreLine = verdict.match(/\*\*(Score [^*]+)\*\*/)?.[1] ?? "Review complete"
  const issueCount = (verdict.match(/^\d+\. \*\*/gm) ?? []).length
  return [
    "Side-Eye review complete",
    "",
    `**${scoreLine}**`,
    `Issues: ${issueCount === 0 ? "none" : issueCount}`,
    `Label: ${label}`,
    `Full verdict: \`/sideeye show\` · Raw: \`${ledger}\``,
  ].join("\n")
}

async function showLastVerdict(context: Context): Promise<void> {
  const path = verdictLedger(context.options)
  if (!existsSync(path)) {
    await context.ui.dialog.alert({
      title: "Side-Eye",
      message: "No stored verdicts yet. Run /sideeye first.",
    })
    return
  }
  const lines = readFileSync(path, "utf8").trim().split("\n").filter(Boolean)
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    try {
      const record = JSON.parse(lines[index])
      if (record?.verdict) {
        await context.ui.dialog.alert({
          title: "Side-Eye verdict",
          message: formatVerdict(record.verdict, path),
        })
        return
      }
    } catch {
      // Skip malformed ledger lines.
    }
  }
  await context.ui.dialog.alert({
    title: "Side-Eye",
    message: "No readable verdict found in the local ledger.",
  })
}

async function labelCurrent(context: Context, label: string): Promise<void> {
  if (!["confirmed", "false-positive", "neutral"].includes(label)) {
    await context.ui.dialog.alert({
      title: "Side-Eye",
      message: "Use /sideeye label confirmed, /sideeye label false-positive, or /sideeye label neutral.",
    })
    return
  }
  try {
    const ledger = await labelLatestVerdict(context.options, label)
    context.ui.toast.show({
      title: "Side-Eye",
      message: `Label saved: ${label} · ${ledger}`,
      variant: "success",
    })
  } catch (error) {
    await context.ui.dialog.alert({
      title: "Side-Eye label failed",
      message: error instanceof Error ? error.message : String(error),
    })
  }
}

function failed(result: string): boolean {
  return [
    "Side-Eye packet build failed",
    "Side-Eye estimate failed",
    "Side-Eye review failed",
    "Side-Eye review could not start",
    "Side-Eye returned no",
  ].some((prefix) => result.startsWith(prefix))
}

async function load(context: Context, sessionID: string) {
  const session = await context.client.session.get({sessionID})
  // OpenCode's sanitizer replaces entire text blocks, not just credentials.
  // Side-Eye redacts the packet itself before judging, so keep the review
  // content readable here.
  const exported = await context.client.session.export({sessionID, sanitize: false})
  return {session, exported}
}

export async function pickAndReview(context: Context, sessionID: string): Promise<void> {
  // Ask for the optional focus text first. This is a real text input; the
  // scope selector below is a filterable list, so typing focus text there only
  // filters the two scope options.
  const instructions = await context.ui.dialog.prompt({
    title: "Side-Eye focus instructions",
    description:
      "Optional. Type what the judge should focus on, then press Enter. Leave blank to use the rubric only.",
    placeholder: "e.g. security, missing tests, API compatibility",
  })
  if (instructions === undefined) return

  const scope = await context.ui.dialog.select<ReviewScope>({
    title: "Side-Eye review scope",
    placeholder: "Choose with arrow keys, then press Enter",
    options: [
      {
        title: "Last exchange",
        value: "last",
        description: "Last request + answer",
      },
      {
        title: "Whole session",
        value: "all",
        description: "All visible context",
      },
    ],
  })
  if (!scope) return

  const command = [scope, instructions.trim()].filter(Boolean).join(" ")

  const preparing = showProgress(context, "Preparing estimate", [
    "Capturing the session and diff.",
    "Checking the judge cost.",
  ])

  let session
  let exported
  try {
    ;({session, exported} = await load(context, sessionID))
  } catch (error) {
    preparing.close()
    await context.ui.dialog.alert({
      title: "Side-Eye",
      message: `The session could not be loaded: ${error instanceof Error ? error.message : String(error)}`,
    })
    return
  }

  const estimate = await reviewExport(session, exported, context.options, command, false)
  preparing.close()
  if (failed(estimate)) {
    await context.ui.dialog.alert({title: "Side-Eye", message: estimate})
    return
  }

  const confirmed = await context.ui.dialog.confirm({
    title: "Run the Side-Eye judge?",
    message: `${estimate}\n\nSend this packet to the judge?`,
    label: {confirm: "Review", cancel: "Cancel"},
  })
  if (!confirmed) {
    context.ui.toast.show({
      title: "Side-Eye",
      message: "Review cancelled.",
      variant: "info",
    })
    return
  }

  const judging = showProgress(context, "Judge reviewing", [
    "The estimate is confirmed.",
    "Waiting for the structured verdict.",
  ])
  const verdict = await reviewExport(session, exported, context.options, `${command} --confirm`)
  judging.close()
  if (failed(verdict)) {
    await context.ui.dialog.alert({title: "Side-Eye", message: verdict})
    return
  }
  const completion = await showVerdict(context, verdict)
  await context.client.session.synthetic({
    sessionID,
    text: reviewCompletion(verdict, completion.label, completion.ledger),
  })
}

export default {
  id: "sideeye.opencode.tui",
  setup(context: Context) {
    return context.ui.slot({
      append: "app",
      render: () => {
        context.keymap.layer(() => ({
          mode: "global",
          commands: [
            {
              id: "sideeye.review",
              title: "Side-Eye review",
              description: "Review the last exchange or the whole session with the judge.",
              group: "Side-Eye",
              palette: true,
              slash: {name: "sideeye", arguments: true},
              enabled: () => context.ui.router.current()?.type === "session",
              run: async (input) => {
                const route = context.ui.router.current()
                if (route.type !== "session") {
                  context.ui.toast.show({
                    title: "Side-Eye",
                    message: "Open a session before requesting a review.",
                    variant: "warning",
                  })
                  return
                }
                if (!input?.trim()) {
                  await pickAndReview(context, route.sessionID)
                  return
                }
                const trimmed = input.trim()
                if (/^show$/i.test(trimmed)) {
                  await showLastVerdict(context)
                  return
                }
                if (/^label\b/i.test(trimmed)) {
                  await labelCurrent(context, trimmed.split(/\s+/, 2)[1]?.toLowerCase() ?? "")
                  return
                }
                if (/^(--help|help)$/i.test(trimmed)) {
                  await context.ui.dialog.alert({title: "Side-Eye", message: USAGE})
                  return
                }
                const progress = showProgress(
                  context,
                  input.includes("--confirm") ? "Judge reviewing" : "Preparing estimate",
                  input.includes("--confirm")
                    ? ["The estimate is confirmed.", "Waiting for the structured verdict."]
                    : ["Capturing the session and diff.", "Checking the judge cost."],
                )
                const result = await reviewExport(
                  await context.client.session.get({sessionID: route.sessionID}),
                  await context.client.session.export({
                    sessionID: route.sessionID,
                    sanitize: false,
                  }),
                  context.options,
                  input,
                )
                progress.close()
                if (input.includes("--confirm")) {
                  const completion = await showVerdict(context, result)
                  await context.client.session.synthetic({
                    sessionID: route.sessionID,
                    text: reviewCompletion(result, completion.label, completion.ledger),
                  })
                } else {
                  await context.client.session.synthetic({sessionID: route.sessionID, text: result})
                }
              },
            },
          ],
        }))
        return null
      },
    })
  },
}
