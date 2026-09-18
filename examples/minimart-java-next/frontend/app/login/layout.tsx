import type { ReactNode } from 'react'

// The chrome-free layout, used by /login. It carries no navigation and no
// balance on purpose: an anonymous visitor has nowhere to navigate to and no
// balance to show, and login.feature asserts the absence of the latter.
export default function BareLayout({ children }: { children: ReactNode }) {
  return (
    <div className="app app--bare">
      <main className="app__main app__main--centred">{children}</main>
    </div>
  )
}
