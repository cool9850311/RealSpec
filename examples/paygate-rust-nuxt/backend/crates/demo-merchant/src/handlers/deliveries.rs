//! `GET /__deliveries` — the addition documented in `demo-merchant.yaml`.
//! Not paygate's log: this merchant's own, so the registry step
//! `merchant received <n> notifications at "<path>"` (`spec/bdd/format.yml`)
//! has something to count against, counted from process start.

use axum::extract::State;
use axum::Json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::state::{AppState, Delivery};

pub async fn get_deliveries(
    State(state): State<Arc<AppState>>,
) -> Json<HashMap<String, Vec<Delivery>>> {
    let deliveries = state.deliveries.read().await;
    Json(deliveries.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::notify;
    use crate::test_support::config_with_gateway;
    use axum::extract::Form;

    #[tokio::test]
    async fn deliveries_are_counted_per_path_from_process_start() {
        let state = Arc::new(AppState::new(config_with_gateway(
            "http://127.0.0.1:1".to_string(),
        )));

        let mut form = std::collections::BTreeMap::new();
        form.insert("MerchantTradeNo".to_string(), "ACME-DELIVERIES".to_string());
        form.insert("RtnMsg".to_string(), "paid".to_string());

        notify::notify(State(state.clone()), Form(form.clone())).await;
        notify::notify(State(state.clone()), Form(form.clone())).await;
        notify::notify_mumble(State(state.clone()), Form(form)).await;

        let Json(deliveries) = get_deliveries(State(state)).await;

        assert_eq!(
            deliveries.get("/demo-merchant/api/notify").unwrap().len(),
            2
        );
        assert_eq!(
            deliveries
                .get("/demo-merchant/api/notify-mumble")
                .unwrap()
                .len(),
            1
        );
        assert!(!deliveries.contains_key("/demo-merchant/api/notify-reject"));

        let recorded = &deliveries.get("/demo-merchant/api/notify").unwrap()[0];
        assert_eq!(recorded.answered, "1|OK");
        assert_eq!(
            recorded.body.get("MerchantTradeNo"),
            Some(&"ACME-DELIVERIES".to_string())
        );
    }
}
