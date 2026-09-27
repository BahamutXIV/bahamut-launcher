# Authentication

[Back to the documentation index](README.md)

The launcher uses this HTTP interface for registration, login, session
validation, and logout. Bahamut owns storage, password hashing, account
lockout, and rate-limit policy.

The launcher presents registration and login inside its own window. It does not
use an external browser or an `ffxiv://login_success` callback.

## Transport

The selected server profile provides `host`, `auth_port`, and `use_https`. The
launcher forms an `/api/v1/` base URL and accepts plain HTTP only for the
loopback hosts `127.0.0.1`, `localhost`, and `[::1]`.

Registration and login send JSON with `Content-Type: application/json`. The
launcher decodes their success bodies and error envelopes. Session validation
uses the HTTP status only, and logout has no request body. Each request has a
10-second timeout. The launcher rejects HTTP redirects and does not follow
`Location` responses.

## Error handling

Action endpoints return this envelope on failure:

```json
{
  "error": {
    "code": "machine-readable-code",
    "message": "human-readable message"
  }
}
```

The launcher recognizes these codes:

| Code | Expected status | Launcher behavior |
|---|---:|---|
| `validation_error` | 400 | Shows the server validation message. |
| `username_taken` | 409 | Shows the registration conflict. |
| `invalid_credentials` | 401 | Shows one generic credential failure. |
| `rate_limited` | 429 | Shows the wait and temporarily disables submission. |
| `server_error` | 500 | Shows a generic server failure. |

Only a 429 response may supply a client-visible `Retry-After` value. Unknown or
malformed responses are reported as server errors. A request that obtains no
HTTP response is reported as a network error.

## Registration

`POST /api/v1/accounts`

```json
{
  "username": "as-supplied",
  "password": "plaintext-utf8"
}
```

The registration page constrains usernames to 3 to 32 characters and passwords
to 8 to 128 characters before submission. The server remains authoritative for
validation. The launcher sends the password only to the selected auth endpoint
and zeroizes its owned credential buffer on drop.

Success is `201 Created`:

```json
{
  "username": "as-supplied",
  "created_at": "<RFC3339 UTC timestamp>"
}
```

`created_at` is RFC 3339 UTC. Registration does not return a session id, so the
launcher follows a successful registration with an explicit login request.

## Login

`POST /api/v1/sessions`

```json
{
  "username": "as-supplied",
  "password": "plaintext-utf8"
}
```

Success is `200 OK`:

```json
{
  "session_id": "56-lowercase-hex-characters",
  "username": "as-supplied",
  "expires_at": "<RFC3339 UTC timestamp>"
}
```

The launcher accepts exactly 56 lowercase hexadecimal session ID characters
and an RFC 3339 UTC expiry. This is a Bahamut API interface. It does not
establish that the retail service used the same token shape. See the
[Handshake](handshake.md#session-token).

## Session validation

`GET /api/v1/sessions/current`

The launcher sends the retained session id only as a bearer token:

```text
Authorization: Bearer <session_id>
```

There is no request body. The HTTP status is authoritative:

| Status | Launcher result |
|---|---|
| `200 OK` | Valid. Keep the retained login. |
| `401 Unauthorized` | Invalid. Return to login. |
| Any other status or transport failure | Unknown. Keep the retained login. |

The launcher does not parse this endpoint's body. An inconclusive validation
never logs the user out.

The launcher stores the issuing authentication endpoint with a remembered
session. Its backend normalizes the URL's scheme, host, effective port, and API
base path. Before validation or game launch, it resolves the saved profile by
its exact display name and compares its current endpoint with that binding.
A removed profile, changed endpoint, or missing binding clears the retained
login without transmitting the token. Login then uses the currently selected
profile. These local invalidations are distinct from a network failure.

Login captures the selected profile and the endpoint that receives the
credentials. If the selected profile, its name, or its authentication endpoint
changes while login is pending, the launcher discards the successful response
and requires a new login. A session is never rebound to a later selection.

## Logout

`DELETE /api/v1/sessions/<session_id>`

Logout has no request body. Any 2xx response succeeds, but revocation is not
guaranteed. The launcher completes local logout even when the endpoint returns
an error, is unavailable, or is not implemented.
The backend checks the issuing endpoint binding before revocation too. A
missing or mismatched binding skips the request while local logout succeeds.

## Launcher rate-limit behavior

The launcher does not implement server rate-limit policy. When login or
registration returns 429, it honors `Retry-After`, displays the remaining wait,
and re-enables submission when the countdown reaches zero. It does not submit a
second request automatically.
