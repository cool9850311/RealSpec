// The API contract, transcribed from spec/openapi/openapi.yaml.
//
// Field names are the wire names (snake_case) on purpose: renaming them in the
// front end would hide a mismatch between this app and the API behind a mapping
// layer, and the whole point of the example is that a mismatch fails loudly.

export type Role = 'guest' | 'admin'

// The two request payloads are `type` rather than `interface` on purpose: only a
// type alias gets TypeScript's implicit index signature, which is what makes it
// assignable to the JSON-body parameter of the fetch helper in lib/api.ts.
export type LoginRequest = {
  username: string
  password: string
}

export interface LoginResponse {
  username: string
  role: Role
}

export interface Me {
  username: string
  role: Role
  points: number
}

export interface Product {
  id: number
  name: string
  cost_points: number
  /** Units remaining, or UNLIMITED_STOCK for unlimited. */
  stock: number
}

export interface ProductList {
  items: Product[]
  total: number
}

export interface Order {
  id: number
  product_name: string
  cost_points: number
  created_at: string
}

export interface OrderList {
  items: Order[]
  total: number
}

export type CreateOrderRequest = {
  product_id: number
}

export interface CreateOrderResponse {
  id: number
  cost_points: number
  balance_after: number
}

export const API_ERROR_CODES = [
  'INVALID_REQUEST',
  'INVALID_CREDENTIALS',
  'UNAUTHENTICATED',
  'PRODUCT_NOT_FOUND',
  'INSUFFICIENT_POINTS',
  'OUT_OF_STOCK',
] as const

export type ApiErrorCode = (typeof API_ERROR_CODES)[number]

export function isApiErrorCode(value: unknown): value is ApiErrorCode {
  return typeof value === 'string' && (API_ERROR_CODES as readonly string[]).includes(value)
}

/** spec.md, "Stock semantics": -1 is the sentinel for unlimited stock. */
export const UNLIMITED_STOCK = -1

/**
 * The readable cookie `POST /auth/login` sets beside the HttpOnly `token`
 * (spec.md, "Authentication"). It is a flag, not a credential: the API never
 * reads it. Its only purpose is to let the SPA tell "signed out" from "signed
 * in" without calling a protected endpoint — see lib/session.tsx.
 */
export const SESSION_HINT_COOKIE = 'session_hint'
