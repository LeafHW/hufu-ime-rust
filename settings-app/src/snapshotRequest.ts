/** A polling/event reader has one request in flight. Actions invalidate any
 * snapshot that started before them; disposed readers never publish results. */
export function createSnapshotRequest<T>(
  load: () => Promise<T>,
  receive: (value: T) => void,
  fail: (error: unknown) => void,
) {
  let generation = 0;
  let disposed = false;
  let pending: Promise<void> | null = null;
  return {
    refresh(reportError = false): Promise<void> {
      if (disposed) return Promise.resolve();
      if (pending) return pending;
      const started = generation;
      pending = Promise.resolve().then(load).then(
        (value) => {
          if (!disposed && started === generation) receive(value);
        },
        (error: unknown) => {
          if (!disposed && started === generation && reportError) fail(error);
        },
      ).finally(() => { pending = null; });
      return pending;
    },
    invalidate() { generation += 1; },
    dispose() { disposed = true; generation += 1; },
  };
}
