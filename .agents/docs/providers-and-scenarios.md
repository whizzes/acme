# Providers and scenarios

Source: specs/000-Initial-Spec.md §9, §10, §11. **Not implemented yet** —
providers land in M2 (Acme Pay/Ship) and M5 (the other eight dialects); see
[[delivery-plan]]. Kept here so M2/M5 don't need to re-read the full spec to
get the provider list and magic values right.

## The one architectural rule (repeat from [[architecture]])

Nothing under `providers/` may write to the database directly. Adapters
parse dialect → core command → `domain`/`db::repo` → render core result back
to dialect.

## Payment providers (spec §10)

| Slug | Display name | Inspired by | Auth | Body format | Flow | Region |
|---|---|---|---|---|---|---|
| `acmepay` | Acme Pay | house style | Bearer `sk_test_…` | JSON | Intent + capture | global |
| `webpay` | Trancorp Webpay | Transbank Webpay Plus | `Tbk-Api-Key-Id` + `Tbk-Api-Key-Secret` headers | JSON | Redirect + explicit commit | CL |
| `pagorapido` | Pago Rápido | Mercado Pago | Bearer `APP_USR-…` | JSON | Preference → payment | BR/AR/MX/CL |
| `chargeflow` | Chargeflow | Stripe Checkout | HTTP Basic, secret key as username | form-urlencoded, bracket notation | Checkout session | global |
| `nordika` | Nordika Pay | invented BNPL/PSD2 | OAuth2 client credentials, JWT bearer | JSON, RFC 9457 errors | Order + SCA challenge + capture | EU |

`acmepay` is the reference dialect: clean REST, JSON, cursor pagination. The
other four exist to make integration code deal with reality (mixed
timestamp formats, decimal vs. minor-unit money, bracket-notation bodies,
RFC 9457 errors).

### Cross-provider capability matrix (for the dashboard's Providers page)

| Capability | Acme Pay | Trancorp Webpay | Pago Rápido | Chargeflow | Nordika |
|---|---|---|---|---|---|
| Hosted checkout | ✅ | ✅ (redirect+commit) | ✅ | ✅ | ✅ (SCA) |
| Direct/server-side charge | ✅ | ❌ | ✅ | ✅ | ❌ |
| Manual capture | ✅ | ✅ (deferred) | ❌ | ✅ | ✅ |
| Partial capture | ✅ | ✅ | ❌ | ✅ | ✅ (multi) |
| Partial refund | ✅ | ✅ | ✅ | ✅ | ✅ |
| Instalments | ✅ | ✅ | ✅ | ❌ | ✅ (plan) |
| Non-card methods | ❌ | ❌ | PIX, boleto, cash | — | invoice, direct debit |
| Webhooks | ✅ | ❌ | ✅ (thin) | ✅ (fat) | ✅ (fat) |
| Idempotency header | ✅ | n/a | ✅ | ✅ | ✅ |
| Money format | minor units | minor units | decimal | minor units | minor units in object |
| Timestamps | RFC3339 Z | mixed | RFC3339 w/ offset | epoch seconds | RFC3339 Z |
| Errors | typed JSON | `error_message` | `cause[]` | typed JSON | RFC 9457 |

## Shipping providers (spec §11)

| Slug | Display name | Inspired by | Auth | Payload language | Speciality |
|---|---|---|---|---|---|
| `acmeship` | Acme Ship | house style | Bearer `sk_test_…` | English | Reference dialect |
| `iberex` | Iberex Express | SEUR | OAuth2 password grant → bearer | Spanish field names | Domestic ES express, `ccc` account codes, ZPL labels |
| `postalis` | Postalis | Correos | Basic + `X-Postalis-Cliente` | Spanish, verbose | National post, pre-registration then label, parcel lockers |
| `shiphub` | ShipHub | Envia.com | Bearer token | English | Multi-carrier aggregator |
| `veloz` | Veloz | invented | HMAC-signed API key | Spanish/Portuguese | Same-day urban courier, live driver position, OTP proof |

Every provider implements **rate**, **create label**, **track**, **cancel**,
plus optional pickups/returns/coverage-lookup/multi-piece.

## Making things go wrong on purpose (spec §9)

Three ways to force an outcome, in increasing precedence:

1. **Magic values** in the request (card PAN, amount, email, postal code).
   Portable, works from any client, survives a DB reset.
2. **`scenario` field** override — every create endpoint accepts
   `metadata.acme_scenario` / `X-Acme-Scenario` header.
3. **Fault rules** in the dashboard (or `POST /admin/faults`), matched by
   provider + method + path glob, with probability and optional fire count.

### Payment magic values

| Trigger | Scenario | Result |
|---|---|---|
| PAN `4111 1111 1111 1111` | `approve` | Captured immediately |
| PAN `5555 5555 5555 4444` | `approve` | Captured, brand mastercard |
| PAN `4000 0000 0000 0002` | `decline_insufficient_funds` | Rejected, response_code `-1` |
| PAN `4000 0000 0000 0069` | `decline_expired_card` | Rejected, response_code `-3` |
| PAN `4000 0000 0000 0127` | `decline_invalid_cvv` | Rejected, response_code `-2` |
| PAN `4000 0000 0000 0119` | `processing_error` | HTTP 502 |
| PAN `4000 0000 0000 3220` | `three_ds_challenge` | Redirect to fake ACS page, then approve |
| PAN `4000 0000 0000 3063` | `three_ds_fail` | Challenge shown, then rejected |
| PAN `4000 0000 0000 0341` | `chargeback` | Captured, then `disputed` after 2 sim days |
| PAN `4000 0000 0000 9995` | `slow_approval` | `pending` 3 sim min, then captured |
| PAN `4242 4242 4242 4241` | `invalid_number` | 400, fails Luhn |
| Amount minor units end `13` | `decline_do_not_honor` | Rejected |
| Amount minor units end `51` | `manual_review` | `pending`, `risk_decision=review`, resolves 10 sim min |
| Amount minor units end `99` | `provider_error` | HTTP 500, no payment created |
| Amount > 5,000,000 minor units | `risk_reject` | Rejected, `risk_rejected` |
| Payer email `decline@acme.test` | `decline_do_not_honor` | Rejected |
| Payer email `slow@acme.test` | `slow_approval` | as above |
| Payer email `chargeback@acme.test` | `chargeback` | as above |
| PIX/boleto, amount ends `77` | `never_paid` | Stays `pending` until expiry |

Test PANs are the industry-standard published test numbers — not real
cards, stored only as brand + last4.

### Shipping magic values

| Trigger | Scenario | Result |
|---|---|---|
| Destination postal code `00000` | `no_coverage` | Rate request → zero options; label creation 422 |
| Postal code starting `999` | `address_exception` | `exception` on first delivery attempt |
| Postal code `07001` (ES) / `7550000` (CL) | `remote_area` | Extra surcharge, +2 days |
| Package > 30,000 g | `oversized` | Rejected at label creation |
| Any dimension > 150 cm | `oversized` | Rejected at label creation |

(Full list continues past this excerpt — read spec §9.2 directly when
implementing shipping scenarios in M5.)

### Fault injection (spec §9.3)

```json
{
  "provider_slug": "chargeflow",
  "method": "POST",
  "path_glob": "/chargeflow/v1/checkout/sessions",
  "mode": "error",
  "http_status": 503,
  "error_code": "api_connection_error",
  "probability": 0.25,
  "remaining": 20,
  "note": "Reproduce ticket ACME-411"
}
```

Modes: `error`, `latency` (add N ms), `timeout` (hold past client patience,
then drop), `malformed` (truncated JSON — good at finding client parser
bugs), `rate_limit` (429 + `Retry-After` + dialect rate-limit headers).

## Quick reference (Appendix A/B/C)

| Provider | Base path | Sample header |
|---|---|---|
| Acme Pay | `/acmepay/v1` | `Authorization: Bearer sk_test_acmepay_demo` |
| Trancorp Webpay | `/webpay/rswebpaytransaction/api/webpay/v1.2` | `Tbk-Api-Key-Id`, `Tbk-Api-Key-Secret` |
| Pago Rápido | `/pagorapido` | `Authorization: Bearer APP_USR-…` |
| Chargeflow | `/chargeflow/v1` | `Authorization: Basic <base64(key + ":")>` |
| Nordika Pay | `/nordika` | `Authorization: Bearer <jwt from /oauth/token>` |
| Acme Ship | `/acmeship/v1` | `Authorization: Bearer …` |
| Iberex | `/iberex/services` | `Authorization: Bearer <oauth2 token>` |
| Postalis | `/postalis/api/v1` | `Authorization: Basic …` |
| ShipHub | `/shiphub/ship` | `Authorization: Bearer shb_test_…` |
| Veloz | `/veloz/v1` | `X-Veloz-Key`, `X-Veloz-Signature` |

Tracking number formats (real check-digit algorithms — mod-11 Iberex-style,
Luhn Acme Ship — so client validators behave as in production):

| Provider | Format | Example |
|---|---|---|
| Acme Ship | `ACM` + 12 digits + check char | `ACM004821993017K` |
| Iberex | 16 digits, first 4 = delegation | `0800000123456789` |
| Postalis | `PQ` + 20 alphanumerics + check | `PQ0012345678901234567X` |
| ShipHub | carrier-native, prefixed | `SH-IBX-0800000123456789` |
| Veloz | `VZ` + 10 base32 | `VZ7K2M9QX4` |

Currency/locale trap worth remembering: **CLP has 0 minor units**. An
integration that assumes "divide by 100" shows Chilean prices 100× too low
— the dashboard should make that obvious immediately once it exists.
