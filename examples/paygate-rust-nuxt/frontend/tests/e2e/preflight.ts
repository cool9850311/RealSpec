// The one thing about the machine this suite cannot work around, checked
// before anything is built or started. Copied unchanged from
// examples/minimart-go-nuxt/frontend/tests/e2e/preflight.ts: the underlying
// Chromium behaviour is the browser's, not the application's, and a stack of
// twelve containers per scenario here starts and stops just as many veths as
// minimart's three did.
//
// Chromium closes every live HTTP/2 session with net::ERR_NETWORK_CHANGED —
// requests in flight included — whenever an address appears on any interface in
// its network namespace. Starting a scenario's stack puts a new veth in the
// Docker host's namespace, and the kernel gives every new device an IPv6
// link-local address a second or two later. On a Linux machine the browser is
// in that same namespace, so a scenario starting while another worker is
// mid-navigation kills that navigation. The page renders and never hydrates,
// and the failure names a timeout rather than a cause.
//
// Setting net.ipv6.conf.default.disable_ipv6 means the address is never
// assigned, so the event is never emitted. `default` is the template for
// interfaces created from that moment on, so Docker's veths are covered and the
// machine's own interfaces are untouched.

import fs from 'node:fs'

const SYSCTL = 'net.ipv6.conf.default.disable_ipv6'
const SYSCTL_FILE = '/proc/sys/net/ipv6/conf/default/disable_ipv6'

/**
 * Refuses to start a run that would be flaky for a reason no test could
 * report. Only a Linux host with a local Docker daemon is exposed, because
 * only there do the browser and the veths share a namespace.
 */
export function assertBrowserSurvivesContainerChurn(): void {
  if (process.platform !== 'linux' || !dockerDaemonIsLocal()) return

  // No file means a kernel built without IPv6, which is the same outcome by a
  // different route: no address to assign, no event to emit.
  if (!fs.existsSync(SYSCTL_FILE)) return

  const value = fs.readFileSync(SYSCTL_FILE, 'utf8').trim()
  if (value === '1') return

  throw new Error(
    `${SYSCTL} is ${value} on this host.\n` +
      '\n' +
      'Every container this suite starts adds a virtual interface, the kernel gives it an\n' +
      'IPv6 link-local address, and Chromium — which shares this namespace — reads that as\n' +
      'the network having changed and aborts every HTTP/2 request in flight with\n' +
      'net::ERR_NETWORK_CHANGED. Twelve containers start per scenario here, so the run\n' +
      'would fail somewhere unrelated to whatever is being tested.\n' +
      '\n' +
      'Keep IPv6 off the interfaces Docker is about to create:\n' +
      '\n' +
      `    sudo sysctl -w ${SYSCTL}=1\n` +
      '\n' +
      'It applies to interfaces created from that moment on, so this machine keeps the IPv6\n' +
      'it already has.',
  )
}

/**
 * Whether the daemon this run talks to is on this machine. An unset
 * DOCKER_HOST means the local socket, and so does an explicit `unix://` or
 * `npipe://` one.
 */
function dockerDaemonIsLocal(): boolean {
  const host = process.env.DOCKER_HOST
  if (!host) return true
  return host.startsWith('unix://') || host.startsWith('npipe://')
}
