// The parts of the container lifecycle testcontainers-node does not expose,
// and nothing else: reading a running container's logs as a finite buffer, for
// a failed scenario's artefact, and listing containers by label, for the
// harness self-test that checks isolation and eventual cleanup.
//
// Deliberately absent: anything that starts, stops or removes a container or a
// network. `spec.md`, "Isolation" gives that job to `testcontainers` alone,
// and unlike examples/minimart-go-nuxt/frontend (which disables Ryuk and
// sweeps its own containers), this suite leaves Ryuk enabled — see
// playwright.config.ts. A `removeNetwork`/`sweepRunResources` pair here would
// be exactly the hand-rolled lifecycle management that section forbids, so
// this file only ever reads the Docker daemon, never writes to it.

import Dockerode from 'dockerode'
import { runId } from './env'

/** The label every container this suite starts carries, for introspection only. */
export const RUN_LABEL = 'realspec.e2e.run'
/** Which scenario, within a run, a container belongs to. */
export const SCENARIO_LABEL = 'realspec.e2e.scenario'

let client: Dockerode | undefined

/** The shared dockerode client, used only for reads. */
export function docker(): Dockerode {
  if (!client) client = new Dockerode()
  return client
}

/** The labels every container of one scenario carries. */
export function scenarioLabels(scenarioIndex: number): Record<string, string> {
  return { [RUN_LABEL]: runId(), [SCENARIO_LABEL]: String(scenarioIndex) }
}

/**
 * The last `tail` lines a container has written, stdout and stderr
 * interleaved.
 *
 * Docker frames a non-TTY log stream in 8-byte headers; without demultiplexing
 * them the artefact would be readable but peppered with control bytes, which
 * is exactly the moment nobody wants to squint at a log.
 */
export async function containerLogs(containerId: string, tail = 2000): Promise<string> {
  try {
    const raw = (await docker().getContainer(containerId).logs({
      stdout: true,
      stderr: true,
      timestamps: false,
      tail,
    })) as unknown as Buffer
    return demultiplex(Buffer.from(raw))
  } catch (error) {
    return `<container logs unavailable: ${describeError(error)}>`
  }
}

function demultiplex(raw: Buffer): string {
  // A stream that does not start with a valid frame header is a TTY stream,
  // which is already plain text.
  const chunks: Buffer[] = []
  let offset = 0
  while (offset + 8 <= raw.length) {
    const streamType = raw[offset]
    if (streamType !== 0 && streamType !== 1 && streamType !== 2) return raw.toString('utf8')
    const length = raw.readUInt32BE(offset + 4)
    const start = offset + 8
    const end = start + length
    if (end > raw.length) break
    chunks.push(raw.subarray(start, end))
    offset = end
  }
  if (chunks.length === 0) return raw.toString('utf8')
  return Buffer.concat(chunks).toString('utf8')
}

export interface RunResources {
  containerIds: string[]
  networkNames: string[]
}

/**
 * Every container still carrying this run's label.
 *
 * Read-only: this is how the harness self-test asserts "Ryuk reaped
 * everything", never how this suite makes that true.
 */
export async function findRunContainers(runIdValue: string): Promise<string[]> {
  const filters = JSON.stringify({ label: [`${RUN_LABEL}=${runIdValue}`] })
  const containers = await docker().listContainers({ all: true, filters })
  return containers.map((c) => c.Id)
}

/**
 * Which of these network names Docker still reports.
 *
 * Matched by name rather than by label because testcontainers-node's
 * `Network` (unlike `GenericContainer`) exposes no `withLabels` in the
 * version this suite pins. Two callers: stack.ts's `ensureRunNetwork`, to
 * decide whether a name a previous invocation recorded still names a real
 * network worth reusing; and the harness self-test, read-only, to confirm the
 * one this run is using is really there.
 */
export async function findExistingNetworks(names: readonly string[]): Promise<string[]> {
  if (names.length === 0) return []
  const all = await docker().listNetworks({ filters: JSON.stringify({ name: [...names] }) })
  const present = new Set(all.map((n) => n.Name))
  return names.filter((name) => present.has(name))
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
