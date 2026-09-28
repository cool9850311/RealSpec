//! The two pages a real browser walks on the e2e surface (`spec.md`, "The
//! payment provider mock", and `spec/bdd/e2e/shop.feature`: a real browser
//! walks the cashier and the issuer page, so those two must be real HTML
//! with real form controls and accessible roles/labels). Every
//! locator `spec/bdd/e2e/shop.feature` uses against these pages is rendered
//! here exactly as written: `heading:Payment`, `textbox:Card number`,
//! `textbox:Expiry`, `textbox:CVC`, `button:Pay`, `button:Cancel`,
//! `heading:3-D Secure`, `testid:issuer-amount`, `button:Authenticate`,
//! `button:Fail authentication` — literal English, never translated, because
//! shop.feature never asserts an `@i18n.key` locator against either page even
//! under `locale is "zh_TW"`.
//!
//! Real navigations, not fragments: `shop.feature`'s "Pressing Back at the
//! provider" scenario relies on the browser's own history, which only a
//! server-rendered document (not a client-side route) produces correctly.

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\n\
         <title>{title}</title></head>\n<body>\n{body}\n</body>\n</html>\n"
    )
}

/// The cashier: a card form, and a way out that never charges anything.
/// `card_endpoint` is the path the card form posts to — the mock accepts the
/// same `CardForm` shape at an ECPay-namespaced and a NewebPay-namespaced
/// path (an addition beyond `payment-provider.yaml`'s one documented path;
/// see this crate's build report), so each cashier's form submits to its own
/// provider's origin rather than to the other provider's endpoint name.
pub fn cashier_page(card_endpoint: &str, provider_trade_no: &str, client_back_url: &str) -> String {
    let provider_trade_no = esc(provider_trade_no);
    let card_endpoint = esc(card_endpoint);
    let client_back_url = esc(client_back_url);
    page(
        "Payment",
        &format!(
            r#"<h1>Payment</h1>
<form method="post" action="{card_endpoint}">
  <input type="hidden" name="providerTradeNo" value="{provider_trade_no}">
  <label for="card_number">Card number</label>
  <input id="card_number" name="card_number" type="text" autocomplete="cc-number">
  <label for="card_expiry">Expiry</label>
  <input id="card_expiry" name="card_expiry" type="text" autocomplete="cc-exp" placeholder="MM/YY">
  <label for="card_cvc">CVC</label>
  <input id="card_cvc" name="card_cvc" type="text" autocomplete="cc-csc">
  <button type="submit">Pay</button>
</form>
<a href="{client_back_url}" role="button">Cancel</a>
"#
        ),
    )
}

/// The provider's own refusal — a `CheckMacValue`/`TradeSha` that did not
/// verify, or a `MerchantTradeNo` reused. Nothing paygate reads (the
/// customer is stuck here, spec/openapi/payment-provider.yaml), so its
/// wording is free; it only has to be an error page rather than a redirect.
pub fn cashier_error_page(message: &str) -> String {
    page(
        "Payment error",
        &format!("<h1>Payment error</h1>\n<p>{}</p>\n", esc(message)),
    )
}

/// The issuing bank's 3-D Secure page — a third origin, offering both
/// outcomes so the same card can take either path (spec.md, "The payment
/// provider mock").
pub fn issuer_page(provider_trade_no: &str, amount_display: &str) -> String {
    let action = format!(
        "/provider/issuer/3ds/{}/authenticate",
        esc(provider_trade_no)
    );
    let amount_display = esc(amount_display);
    page(
        "3-D Secure",
        &format!(
            r#"<h1>3-D Secure</h1>
<p data-testid="issuer-amount">{amount_display}</p>
<form method="post" action="{action}">
  <input type="hidden" name="outcome" value="authenticate">
  <button type="submit">Authenticate</button>
</form>
<form method="post" action="{action}">
  <input type="hidden" name="outcome" value="fail">
  <button type="submit">Fail authentication</button>
</form>
"#
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cashier_page_carries_every_locator_shop_feature_needs() {
        let html = cashier_page(
            "/provider/ecpay/Cashier/Card",
            "ACME00000001SN4f2a1",
            "/shop/result",
        );
        assert!(html.contains("<h1>Payment</h1>"));
        assert!(html.contains(">Card number<"));
        assert!(html.contains(">Expiry<"));
        assert!(html.contains(">CVC<"));
        assert!(html.contains(">Pay<"));
        assert!(html.contains(">Cancel<"));
        assert!(html.contains("action=\"/provider/ecpay/Cashier/Card\""));
        assert!(html.contains("value=\"ACME00000001SN4f2a1\""));
        assert!(html.contains("href=\"/shop/result\""));
    }

    #[test]
    fn issuer_page_carries_the_amount_and_both_outcomes() {
        let html = issuer_page("ACME00000001SN4f2a1", "$12.50");
        assert!(html.contains("<h1>3-D Secure</h1>"));
        assert!(html.contains("data-testid=\"issuer-amount\">$12.50<"));
        assert!(html.contains(">Authenticate<"));
        assert!(html.contains(">Fail authentication<"));
        assert!(html.contains("/provider/issuer/3ds/ACME00000001SN4f2a1/authenticate"));
    }

    #[test]
    fn html_is_escaped() {
        let html = cashier_error_page("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
    }
}
