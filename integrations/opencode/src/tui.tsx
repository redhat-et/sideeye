import { Plugin } from "@opencode/plugin/tui"

export default Plugin.define({
  id: "sideeye.opencode.tui",
  setup(context: any) {
    context.keymap.layer(() => ({
      mode: "global",
      commands: [
        {
          id: "sideeye.review",
          title: "Side-Eye review",
          group: "Side-Eye",
          palette: true,
          slash: { name: "sideeye", aliases: ["sideeye-review"], arguments: false },
          enabled: () => context.ui.router.current()?.type === "session",
          run: async () => {
            const route = context.ui.router.current() as { type?: string; sessionID?: string } | undefined
            if (!route?.sessionID) {
              context.ui.toast.show({
                title: "Side-Eye",
                message: "Open a session before requesting a review.",
                variant: "warning",
              })
              return
            }
            await context.client.session.command({
              sessionID: route.sessionID,
              command: "sideeye-review",
              arguments: "",
            })
          },
        },
      ],
    }))
  },
})
