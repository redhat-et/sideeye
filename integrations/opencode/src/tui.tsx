import type { Context } from "@opencode/plugin/tui/context"
import { reviewExport, type ReviewScope } from "./index.js"

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
        description: "The last request and the assistant work that followed it.",
      },
      {
        title: "Whole session",
        value: "all",
        description: "All visible context in this session.",
      },
    ],
  })
  if (!scope) return

  const command = [scope, instructions.trim()].filter(Boolean).join(" ")

  let session
  let exported
  try {
    ;({session, exported} = await load(context, sessionID))
  } catch (error) {
    await context.ui.dialog.alert({
      title: "Side-Eye",
      message: `The session could not be loaded: ${error instanceof Error ? error.message : String(error)}`,
    })
    return
  }

  const estimate = await reviewExport(session, exported, context.options, command, false)
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

  const verdict = await reviewExport(session, exported, context.options, `${command} --confirm`)
  if (failed(verdict)) {
    await context.ui.dialog.alert({title: "Side-Eye", message: verdict})
    return
  }
  await context.client.session.synthetic({sessionID, text: verdict})
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
                if (/^(--help|help)$/i.test(input.trim())) {
                  await context.ui.dialog.alert({title: "Side-Eye", message: USAGE})
                  return
                }
                const result = await reviewExport(
                  await context.client.session.get({sessionID: route.sessionID}),
                  await context.client.session.export({
                    sessionID: route.sessionID,
                    sanitize: false,
                  }),
                  context.options,
                  input,
                )
                await context.client.session.synthetic({sessionID: route.sessionID, text: result})
              },
            },
          ],
        }))
        return null
      },
    })
  },
}
