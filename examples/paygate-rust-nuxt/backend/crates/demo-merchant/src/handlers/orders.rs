//! `POST /orders`, `GET /orders/{no}/form` and `GET /orders/{no}` —
//! `demo-merchant.yaml`'s shop-facing surface, called directly by the
//! browser (`frontend/composables/useShopApi.ts`).

use axum::extract::{Path, State};
use axum::Json;
use std::sync::Arc;
use uuid::Uuid;

use crate::amount::parse_decimal_to_minor_units;
use crate::dto::{CreateShopOrderRequest, CreateShopOrderResponse, ShopOrder, ShopPaymentForm};
use crate::error::AppError;
use crate::gateway::CreatePaymentParams;
use crate::state::{AppState, Order, OrderStatus};

/// This demo seeds exactly one merchant (`Acme Coffee`, `USD` —
/// `shop.feature`'s Background), so the shop's own currency and item
/// description are fixed the way a real storefront's templates would be,
/// without asking an API for them (mirrors `frontend/utils/shop-session.ts`,
/// `SHOP_CURRENCY`).
const SHOP_CURRENCY: &str = "USD";
const SHOP_ITEM_DESC: &str = "Shop order";
const SHOP_NOTIFY_URL: &str = "/demo-merchant/api/notify";
const SHOP_CLIENT_BACK_URL: &str = "/shop/result";

/// A merchant order number this merchant has not used before, `<= 20` chars,
/// `[A-Za-z0-9-]` only (`openapi.yaml`, `CreateOrderRequest.merchant_trade_no`).
/// A UUIDv7's leading bytes are time-ordered, which is a readable-enough
/// order number and, in the birthday-bound sense a single demo process cares
/// about, as good as any other source of local uniqueness.
fn generate_merchant_trade_no() -> String {
    let raw = Uuid::now_v7().simple().to_string();
    format!("SHOP-{}", &raw[..14])
}

/// `POST /orders`: allocate an order number, ask paygate to create the order
/// and sign a form for it, and hand the shop its OWN payment page — never
/// paygate's, which serves none.
pub async fn create_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShopOrderRequest>,
) -> Result<(axum::http::StatusCode, Json<CreateShopOrderResponse>), AppError> {
    let amount_minor =
        parse_decimal_to_minor_units(&req.amount).map_err(|_| AppError::InvalidRequest)?;
    let merchant_trade_no = generate_merchant_trade_no();

    let payment = state
        .gateway
        .create_payment(&CreatePaymentParams {
            merchant_trade_no: &merchant_trade_no,
            amount_minor,
            currency: SHOP_CURRENCY,
            item_desc: SHOP_ITEM_DESC,
            notify_url: SHOP_NOTIFY_URL,
            client_back_url: SHOP_CLIENT_BACK_URL,
        })
        .await
        .map_err(|_| AppError::GatewayUnavailable)?;

    let order = Order {
        merchant_trade_no: merchant_trade_no.clone(),
        payment_id: payment.id,
        amount_minor,
        currency: SHOP_CURRENCY.to_string(),
        item_desc: SHOP_ITEM_DESC.to_string(),
        notify_url: SHOP_NOTIFY_URL.to_string(),
        client_back_url: SHOP_CLIENT_BACK_URL.to_string(),
        status: payment.status.into(),
        action: payment.action.unwrap_or_default(),
        fields: payment.fields.unwrap_or_default(),
        // The form just minted has not been handed to the shop's own page
        // yet — that happens on the FIRST `GET .../form`, which is why this
        // call does not count as "asking again".
        form_issued: false,
    };

    state
        .orders
        .write()
        .await
        .insert(merchant_trade_no.clone(), order);

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateShopOrderResponse {
            pay_url: format!("/shop/pay/{merchant_trade_no}"),
            merchant_trade_no,
        }),
    ))
}

/// `GET /orders/{no}/form`: what the shop's own `/shop/pay/{no}` page must
/// submit.
///
/// `POST /orders` already opened one attempt at paygate and signed one form
/// for it — paygate's `POST /payments` always does both in the same call,
/// there is no way to have an order without an attempt. So the FIRST time
/// this is asked, it simply hands out that already-signed form; only asking
/// **again** (`shopGetPaymentForm`: "Asking again mints a new one") calls
/// paygate a second time, with the SAME `merchant_trade_no` and parameters
/// but a fresh `Idempotency-Key`, which is what makes ECPay's own rule
/// ("never reuse a provider order number") mint a new `provider_trade_no`.
/// One page render costs exactly one attempt — the invariant
/// `shop.feature`'s `payment_attempts` counts assert after every "render the
/// pay page N times" scenario.
pub async fn get_payment_form(
    State(state): State<Arc<AppState>>,
    Path(merchant_trade_no): Path<String>,
) -> Result<Json<ShopPaymentForm>, AppError> {
    // The claim for "the first ask" happens under one write lock, with no
    // `.await` in between check and set, so two concurrent first asks
    // cannot both be told they were first.
    {
        let mut orders = state.orders.write().await;
        let order = orders
            .get_mut(&merchant_trade_no)
            .ok_or(AppError::NotFound)?;
        if order.status != OrderStatus::AwaitingPayment {
            return Err(AppError::AlreadySettled);
        }
        if !order.form_issued {
            order.form_issued = true;
            return Ok(Json(ShopPaymentForm {
                action: order.action.clone(),
                fields: order.fields.clone(),
            }));
        }
    }

    // Asking again: mint a new one.
    let existing = {
        let orders = state.orders.read().await;
        orders
            .get(&merchant_trade_no)
            .cloned()
            .ok_or(AppError::NotFound)?
    };
    if existing.status != OrderStatus::AwaitingPayment {
        return Err(AppError::AlreadySettled);
    }

    let payment = state
        .gateway
        .create_payment(&CreatePaymentParams {
            merchant_trade_no: &existing.merchant_trade_no,
            amount_minor: existing.amount_minor,
            currency: &existing.currency,
            item_desc: &existing.item_desc,
            notify_url: &existing.notify_url,
            client_back_url: &existing.client_back_url,
        })
        .await
        .map_err(|_| AppError::GatewayUnavailable)?;

    let action = payment.action.unwrap_or_default();
    let fields = payment.fields.unwrap_or_default();

    {
        let mut orders = state.orders.write().await;
        if let Some(order) = orders.get_mut(&merchant_trade_no) {
            order.status = payment.status.into();
            order.action = action.clone();
            order.fields = fields.clone();
            order.form_issued = true;
        }
    }

    Ok(Json(ShopPaymentForm { action, fields }))
}

/// `GET /orders/{no}`: the merchant's own belief about the order. `status`
/// changes only through a `notify-*` callback. `last_attempt_status` is the
/// addition documented in `demo-merchant.yaml`: asked of paygate only while
/// this merchant's own record is still `awaiting_payment`, because that is
/// the one fact a failed attempt never arrives here by itself
/// (`spec.md`, "A failed attempt leaves the order payable and tells the
/// merchant nothing").
pub async fn get_order(
    State(state): State<Arc<AppState>>,
    Path(merchant_trade_no): Path<String>,
) -> Result<Json<ShopOrder>, AppError> {
    let order = {
        let orders = state.orders.read().await;
        orders
            .get(&merchant_trade_no)
            .cloned()
            .ok_or(AppError::NotFound)?
    };

    let mut last_attempt_status = None;
    if order.status == OrderStatus::AwaitingPayment {
        if let Ok(payment) = state.gateway.get_payment(&order.payment_id).await {
            last_attempt_status = payment.last_attempt.map(|a| a.status);
        }
    }

    Ok(Json(ShopOrder {
        merchant_trade_no: order.merchant_trade_no,
        status: order.status,
        amount: order.amount_minor,
        trade_no: Some(order.payment_id),
        last_attempt_status,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{config_with_gateway, spawn_failing_gateway, spawn_fake_gateway};
    use paygate_domain::AttemptStatus;

    #[tokio::test]
    async fn creating_an_order_converts_the_decimal_amount_and_returns_its_own_pay_url() {
        let gateway =
            spawn_fake_gateway(serde_json::json!({ "id": "unused", "status": "pending" })).await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url)));

        let (status, Json(body)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "12.50".to_string(),
            }),
        )
        .await
        .expect("order creation should succeed against a healthy gateway");

        assert_eq!(status, axum::http::StatusCode::CREATED);
        assert_eq!(
            body.pay_url,
            format!("/shop/pay/{}", body.merchant_trade_no)
        );

        let orders = state.orders.read().await;
        let stored = orders
            .get(&body.merchant_trade_no)
            .expect("the order was recorded");
        assert_eq!(stored.amount_minor, 1250);
        assert_eq!(stored.status, OrderStatus::AwaitingPayment);
    }

    #[tokio::test]
    async fn an_amount_that_does_not_match_the_decimal_pattern_never_reaches_the_gateway() {
        let state = Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )));

        let result = create_order(
            State(state),
            Json(CreateShopOrderRequest {
                amount: "not-a-number".to_string(),
            }),
        )
        .await;

        assert!(matches!(result, Err(AppError::InvalidRequest)));
    }

    /// `spec/openapi/demo-merchant.yaml`, `POST /orders`: "paygate could not
    /// be reached or refused the order" is `502 GATEWAY_UNAVAILABLE`.
    /// Required by `spec.md`'s Test plan ("the 502 path").
    #[tokio::test]
    async fn a_refusing_gateway_makes_order_creation_502() {
        let gateway_url = spawn_failing_gateway().await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway_url)));

        let result = create_order(
            State(state),
            Json(CreateShopOrderRequest {
                amount: "9.00".to_string(),
            }),
        )
        .await;

        assert!(matches!(result, Err(AppError::GatewayUnavailable)));
    }

    /// The same 502 path, exercised through `GET /orders/{no}/form`'s own
    /// mint call — which only ever happens on the SECOND ask, since the
    /// first one just hands out the form `POST /orders` already has.
    #[tokio::test]
    async fn a_gateway_that_starts_refusing_makes_re_minting_the_form_502() {
        let gateway = spawn_fake_gateway(serde_json::json!({ "status": "pending" })).await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url.clone())));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();

        // Consume the free first ask against the still-healthy gateway, so
        // what is left in the order's own record is "already issued" —
        // exactly the state a second ask needs to trigger a real mint.
        let _first_ask = get_payment_form(
            State(state.clone()),
            Path(created.merchant_trade_no.clone()),
        )
        .await
        .expect("the first ask never touches the gateway");

        // The fake gateway is not stopped here; instead a SECOND state is
        // pointed at an address nothing listens on, standing in for the
        // gateway going away between the first ask and the next one.
        let unreachable = Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )));
        unreachable.orders.write().await.insert(
            created.merchant_trade_no.clone(),
            state
                .orders
                .read()
                .await
                .get(&created.merchant_trade_no)
                .unwrap()
                .clone(),
        );

        let result = get_payment_form(State(unreachable), Path(created.merchant_trade_no)).await;
        assert!(matches!(result, Err(AppError::GatewayUnavailable)));
    }

    #[tokio::test]
    async fn asking_for_the_form_of_an_unknown_order_is_404() {
        let state = Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )));
        let result = get_payment_form(State(state), Path("NO-SUCH-ORDER".to_string())).await;
        assert!(matches!(result, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn asking_for_the_form_of_a_paid_order_is_409() {
        let gateway = spawn_fake_gateway(serde_json::json!({ "status": "pending" })).await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url)));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();

        {
            let mut orders = state.orders.write().await;
            orders.get_mut(&created.merchant_trade_no).unwrap().status = OrderStatus::Paid;
        }

        let result = get_payment_form(State(state), Path(created.merchant_trade_no)).await;
        assert!(matches!(result, Err(AppError::AlreadySettled)));
    }

    /// `POST /orders` already opened one attempt at paygate and signed one
    /// form for it (that IS what `POST /api/v1/payments` does in one call).
    /// So the FIRST `GET .../form` must hand that same form out rather than
    /// minting a second attempt nobody asked for — `shop.feature`'s own
    /// attempt-count assertions ("one page render, one attempt") are exactly
    /// this invariant.
    #[tokio::test]
    async fn the_first_ask_hands_out_the_form_from_creation_without_minting_again() {
        let gateway = spawn_fake_gateway(serde_json::json!({ "status": "pending" })).await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url.clone())));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            gateway.create_calls(),
            1,
            "creating the order is the only mint so far"
        );

        let form = get_payment_form(
            State(state.clone()),
            Path(created.merchant_trade_no.clone()),
        )
        .await
        .expect("the first ask always has a form to hand out");

        assert_eq!(
            gateway.create_calls(),
            1,
            "the first ask must not mint again"
        );
        assert_eq!(form.action, "/provider/ecpay/Cashier/AioCheckOut/V5");
    }

    /// Pins the exact arithmetic `shop.feature` ("Rendering the payment page
    /// twice", "A customer who fails authentication can try again") checks
    /// against `payment_attempts`: two successive asks for the form, after
    /// creation, must cost exactly ONE additional mint at paygate — not two,
    /// and not zero — and the two forms handed out must be for two
    /// genuinely different provider orders.
    #[tokio::test]
    async fn asking_for_the_form_a_second_time_mints_a_new_one_with_a_different_provider_trade_no()
    {
        let gateway = spawn_fake_gateway(serde_json::json!({ "status": "pending" })).await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url.clone())));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();

        let first = get_payment_form(
            State(state.clone()),
            Path(created.merchant_trade_no.clone()),
        )
        .await
        .expect("the first ask hands out the form from creation");
        assert_eq!(
            gateway.create_calls(),
            1,
            "the first of the two successive asks must not mint again"
        );

        let second = get_payment_form(
            State(state.clone()),
            Path(created.merchant_trade_no.clone()),
        )
        .await
        .expect("asking again mints a new one");
        assert_eq!(
            gateway.create_calls(),
            2,
            "two successive asks after creation must cost exactly ONE additional mint"
        );

        assert_ne!(
            first.fields.get("MerchantTradeNo"),
            second.fields.get("MerchantTradeNo"),
            "two mints must carry two different provider trade numbers"
        );
    }

    #[tokio::test]
    async fn asking_for_an_unknown_order_is_404() {
        let state = Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )));
        let result = get_order(State(state), Path("NO-SUCH-ORDER".to_string())).await;
        assert!(matches!(result, Err(AppError::NotFound)));
    }

    /// The addition documented in `demo-merchant.yaml`:
    /// `GET /orders/{no}` asks paygate for `last_attempt` while its own
    /// record is still `awaiting_payment`, and reports it through.
    #[tokio::test]
    async fn an_awaiting_order_reports_a_failed_last_attempt_from_paygates_own_query() {
        // Starts with nothing to report — no card has been tried yet — then
        // the test arms the fake gateway's answer the way a real customer's
        // failed 3-D Secure attempt would change it, and asks again.
        let gateway = spawn_fake_gateway(serde_json::json!({
            "id": "paygate-id-does-not-matter-here",
            "status": "pending",
        }))
        .await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url.clone())));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();

        let before = get_order(
            State(state.clone()),
            Path(created.merchant_trade_no.clone()),
        )
        .await
        .expect("a known order is always readable");
        assert_eq!(before.last_attempt_status, None);

        gateway.set_get_response(serde_json::json!({
            "id": "paygate-id-does-not-matter-here",
            "status": "pending",
            "last_attempt": { "provider": "ecpay", "status": "failed" }
        }));

        let after = get_order(State(state), Path(created.merchant_trade_no))
            .await
            .expect("a known order is always readable");

        assert_eq!(after.status, OrderStatus::AwaitingPayment);
        assert_eq!(after.last_attempt_status, Some(AttemptStatus::Failed));
    }

    /// Once the merchant's OWN record says `paid`, it does not need to ask
    /// paygate anything more — `spec/openapi/demo-merchant.yaml`,
    /// `GET /orders/{merchantTradeNo}`: this endpoint asks paygate itself
    /// only "while this merchant's own `status` is still
    /// `awaiting_payment`".
    #[tokio::test]
    async fn a_paid_order_never_asks_paygate_for_a_last_attempt() {
        let gateway = spawn_fake_gateway(serde_json::json!({
            "status": "pending",
            "last_attempt": { "provider": "ecpay", "status": "failed" }
        }))
        .await;
        let state = Arc::new(AppState::new(config_with_gateway(gateway.base_url)));

        let (_, Json(created)) = create_order(
            State(state.clone()),
            Json(CreateShopOrderRequest {
                amount: "5.00".to_string(),
            }),
        )
        .await
        .unwrap();

        {
            let mut orders = state.orders.write().await;
            orders.get_mut(&created.merchant_trade_no).unwrap().status = OrderStatus::Paid;
        }

        let order = get_order(State(state), Path(created.merchant_trade_no))
            .await
            .unwrap();
        assert_eq!(order.status, OrderStatus::Paid);
        assert_eq!(order.last_attempt_status, None);
    }
}
