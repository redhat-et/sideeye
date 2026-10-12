import { Plugin } from "@opencode/plugin/tui"
import type { Context } from "@opencode/plugin/tui/context"
import { reviewExport } from "./index.js"

export default Plugin.define({
  id: "sideeye.opencode.tui",
  setup(context: Context) {
    context.keymap.layer(() => ({
      mode: "global",
      commands: [
        {
          id: "sideeye.review",
          title: "Side-Eye review",
          group: "Side-Eye",
          palette: true,
          slash: { name: "sideeye", aliases: ["sideeye-review"], arguments: true },
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
            const session = await context.client.session.get({sessionID: route.sessionID})
            const exported = await context.client.session.export({
              sessionID: route.sessionID,
              sanitize: true,
            })
            const result = await reviewExport(session, exported, context.options, input ?? "")
            await context.client.session.synthetic({sessionID: route.sessionID, text: result})
          },
        },
      ],
    }))
  },
})
