/* amesh-plugin:opencode:v1 */
export default {
  id: "amesh",
  setup(ctx) {
    const stop = new AbortController();
    const joined = new Map();
    let pending = false;
    let generation = 0;
    let retryAt = 0;
    let attempts = 0;
    let previous;
    async function JoinCurrent() {
      if (pending || stop.signal.aborted) return;
      const route = ctx.ui.router.current();
      if (route.type !== "session") return;
      const sessionID = ctx.data.session.root(route.sessionID);
      const info = ctx.data.session.get(sessionID);
      if (!info || info.parentID) return;
      if (Date.now() < (joined.get(sessionID)?.until ?? 0)) return;
      if (previous !== sessionID) {
        previous = sessionID;
        attempts = 0;
        retryAt = 0;
      }
      if (Date.now() < retryAt) return;
      pending = true;
      const currentGeneration = generation;
      try {
        const waiting = ctx.data.session
          .family(sessionID)
          .some(
            (id) => (ctx.data.session.permission.list(id) ?? []).length > 0,
          );
        const state = waiting
          ? "wait"
          : ctx.data.session.status(sessionID) === "running"
            ? "work"
            : "idle";
        await ctx.client.rpc.call(
          {
            rpcID: "amesh",
            method: "join",
            input: { sessionID, state },
            location: { directory: info.location.directory },
          },
          { signal: stop.signal },
        );
        if (!stop.signal.aborted && generation === currentGeneration)
          joined.set(sessionID, {
            directory: info.location.directory,
            until: Date.now() + 30000,
          });
        attempts = 0;
      } catch (error) {
        if (!stop.signal.aborted) {
          if (
            !attempts ||
            error?.type === "stopped" ||
            error?.type === "location"
          )
            ctx.ui.toast.show({
              variant: "error",
              message: `amesh: ${error?.message || error?.type || "could not join this session"}`,
            });
          if (error?.type === "stopped" || error?.type === "location") {
            if (generation === currentGeneration)
              joined.set(sessionID, {
                directory: info.location.directory,
                until: Infinity,
              });
            return;
          }
          retryAt =
            Date.now() + Math.min(30000, 1000 * 2 ** Math.min(attempts++, 5));
        }
      } finally {
        pending = false;
      }
    }
    const unlisten = ctx.data.listen(({ details: event }) => {
      if (event.type === "server.connected") {
        generation++;
        joined.clear();
      }
      if (event.type === "location.shutdown") {
        generation++;
        for (const [id, entry] of joined)
          if (entry.directory === event.location?.directory) joined.delete(id);
      }
    });
    const timer = setInterval(() => void JoinCurrent(), 500);
    timer.unref?.();
    void JoinCurrent();
    return () => {
      stop.abort();
      clearInterval(timer);
      unlisten();
    };
  },
};
