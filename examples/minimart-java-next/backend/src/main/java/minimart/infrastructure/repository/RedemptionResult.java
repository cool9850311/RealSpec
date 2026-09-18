package minimart.infrastructure.repository;

import minimart.domain.Order;

/**
 * What a committed redemption produced: the order row as inserted, and the caller's balance after
 * the deduction.
 */
public record RedemptionResult(Order order, int balanceAfter) {}
