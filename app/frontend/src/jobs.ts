export interface PollJobCallbacks {
  onProgress: (completed: number, total: number, pct: number) => void
  onDone: () => void
  onError: (msg: string) => void
}

/**
 * Polls GET /jobs/:id every 4s until the job reaches a terminal state.
 * Returns a stop function; call it to cancel the next scheduled tick.
 */
export function pollJob(jobId: string, total: number, callbacks: PollJobCallbacks): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null

  function stop() {
    if (timer !== null) { clearTimeout(timer); timer = null }
  }

  function tick() {
    timer = setTimeout(async () => {
      try {
        const res = await fetch(`/jobs/${jobId}`)
        if (!res.ok) {
          callbacks.onError(`Job not found (${res.status}) — server may have restarted`)
          return
        }
        const job = await res.json()
        if (job.status === 'done') {
          callbacks.onDone()
          return
        }
        if (job.status === 'error') {
          callbacks.onError(`Synthesis error: ${job.error ?? ''}`)
          return
        }
        if (job.status === 'cancelled') {
          callbacks.onError('Job cancelled.')
          return
        }
        const pct = total > 0 ? Math.round((job.completed_sentences / total) * 100) : 0
        callbacks.onProgress(job.completed_sentences, total, pct)
        tick()
      } catch {
        tick() // retry silently on network blip
      }
    }, 4000)
  }

  tick()
  return stop
}
