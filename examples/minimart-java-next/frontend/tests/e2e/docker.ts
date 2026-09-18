// The parts of the container lifecycle testcontainers-node does not expose:
// reading a running container's logs as a finite buffer, for a failed scenario's
// artefact, and creating a network that carries this run's label. Labelled
// networks and labelled containers are what let sweepRunResources find this
// suite's leftovers and nothing else on the machine.

import Dockerode from 'dockerode'
import { RUN_LABEL } from './env'

let client: Dockerode | undefined

/** The shared dockerode client. */
export function docker(): Dockerode {
  if (!client) client = new Dockerode()
  return client
}

/** The label set every container and network of this run carries. */
export function runLabels(runId: string): Record<string, string> {
  return { [RUN_LABEL]: runId }
}

/** Creates a labelled bridge network and returns its name. */
export async function createNetwork(name: string, runId: string): Promise<string> {
  await docker().createNetwork({
    Name: name,
    Driver: 'bridge',
    CheckDuplicate: true,
    Labels: runLabels(runId),
  })
  return name
}

/**
 * Disconnects a container from a network, ignoring a network that is already
 * gone. `Force` lets this succeed even when the Engine's own view of the
 * container is out of date — exactly the state a container removed by other
 * means (`docker rm -f`, an OOM kill) can leave behind.
 */
async function disconnectFromNetwork(networkName: string, containerId: string): Promise<void> {
  try {
    await docker().getNetwork(networkName).disconnect({ Container: containerId, Force: true })
  } catch (error) {
    if (!isNotFound(error)) throw error
  }
}

/**
 * Removes a network, ignoring one that is already gone.
 *
 * A network with an endpoint still attached refuses removal outright, so this
 * disconnects what is left first. That is what makes it safe to call from
 * sweepRunResources: a worker that died before its `finally` ran leaves its
 * containers attached to the run's network, and the sweep must still be able to
 * take the network down afterwards.
 */
export async function removeNetwork(name: string): Promise<void> {
  try {
    await docker().getNetwork(name).remove()
  } catch (error) {
    if (isNotFound(error)) return
    if (!isHasActiveEndpoints(error)) throw error
    await forceDisconnectAll(name)
    await docker().getNetwork(name).remove()
  }
}

/** Disconnects every container still attached to a network. */
async function forceDisconnectAll(name: string): Promise<void> {
  const info = await docker().getNetwork(name).inspect()
  const containerIds = Object.keys(info.Containers ?? {})
  for (const containerId of containerIds) {
    await disconnectFromNetwork(name, containerId)
  }
}

/**
 * The last `tail` lines a container has written, stdout and stderr interleaved.
 *
 * Docker frames a non-TTY log stream in 8-byte headers; without demultiplexing
 * them the artefact would be readable but peppered with control bytes, which is
 * exactly the moment nobody wants to squint at a log.
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

/** Every container and network still carrying this run's label. */
export async function findRunResources(runId: string): Promise<RunResources> {
  const filters = JSON.stringify({ label: [`${RUN_LABEL}=${runId}`] })
  const containers = await docker().listContainers({ all: true, filters })
  const networks = await docker().listNetworks({ filters })
  return {
    containerIds: containers.map((c) => c.Id),
    networkNames: networks.map((n) => n.Name),
  }
}

/**
 * Removes everything still carrying this run's label.
 *
 * Every scenario tears its own stack down in a `finally`, so in a healthy run
 * this finds nothing. It exists for the run that did not stay healthy — a
 * worker killed mid-scenario leaves containers that no `finally` will ever
 * reach, and a suite that starts containers owes the machine a guarantee that
 * it stops them.
 */
export async function sweepRunResources(runId: string): Promise<RunResources> {
  const found = await findRunResources(runId)
  for (const id of found.containerIds) {
    try {
      await docker().getContainer(id).remove({ force: true, v: true })
    } catch (error) {
      if (!isNotFound(error)) throw error
    }
  }
  for (const name of found.networkNames) {
    await removeNetwork(name)
  }
  return found
}

function isNotFound(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as { statusCode?: number }).statusCode === 404
}

/** The Engine's response to removing a network that still has an endpoint on it. */
function isHasActiveEndpoints(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as { statusCode?: number }).statusCode === 403
}

export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
