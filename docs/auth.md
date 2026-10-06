# Authentication

[Back to the documentation index](README.md)

Use this HTTP API to register accounts, log in, validate sessions, and log out.
Bahamut handles storage, password hashing, account lockout, and rate limits.

Registration and login run inside the launcher window. There is no external
browser flow or `ffxiv://login_success` callback.

## Transport

The selected server profile supplies `host`, `auth_port`, and `use_https`.
The launcher builds an `/api/v1/` base URL. It permits plain HTTP only for
the loopback hosts `127.0.0.1`, `localhost`, and `[::1]`.

Registration and login send JSON with `Content-Type: application/json` and
decode both success bodies and error envelopes. Session validation reads only
the HTTP status. Logout sends no body. Each request times out after 10 seconds.
The launcher rejects redirects and never follows `Location` responses.

## Error handling

Action endpoints return this error envelope:

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
| `validation_error` | 400 | Shows the server's validation message. |
| `username_taken` | 409 | Shows the registration conflict. |
| `invalid_credentials` | 401 | Shows a generic credential failure. |
| `rate_limited` | 429 | Shows the wait and temporarily disables submission. |
| `server_error` | 500 | Shows a generic server failure. |

Only a 429 response can provide a client-visible `Retry-After` value. Unknown
or malformed responses become server errors. A request that receives no HTTP
response becomes a network error.

## Registration

`POST /api/v1/accounts`

```json
{
  "username": "as-supplied",
  "password": "plaintext-utf8"
}
```

Before submission, the registration page requires a username of 3 to 32
characters and a password of 8 to 128 characters. The server makes the final
validation decision. The launcher sends the password only to the selected
auth endpoint and zeroizes its own credential buffer when dropped.

Return `201 Created` on success:

```json
{
  "username": "as-supplied",
  "created_at": "<RFC3339 UTC timestamp>"
}
```

`created_at` must be RFC 3339 UTC. Registration returns no session ID, so the
launcher makes a separate login request after registration succeeds.

## Login

`POST /api/v1/sessions`

```json
{
  "username": "as-supplied",
  "password": "plaintext-utf8"
}
```

Return `200 OK` on success:

```json
{
  "session_id": "56-lowercase-hex-characters",
  "username": "as-supplied",
  "expires_at": "<RFC3339 UTC timestamp>"
}
```

The launcher requires exactly 56 lowercase hexadecimal characters in the
session ID and an RFC 3339 UTC expiry. These are Bahamut API requirements;
the retail service's token shape is unverified. See
[Handshake](handshake.md#session-token).

## Session validation

`GET /api/v1/sessions/current`

The launcher sends the retained session ID only as a bearer token:

```text
Authorization: Bearer <session_id>
```

The request has no body. The launcher ignores the response body and uses the
status alone:

| Status | Launcher result |
|---|---|
| `200 OK` | Valid. Keep the retained login. |
| `401 Unauthorized` | Invalid. Return to login. |
| Any other status or transport failure | Unknown. Keep the retained login. |

An inconclusive validation never logs the user out.

A remembered session includes its issuing authentication endpoint. The backend
normalizes the URL's scheme, host, effective port, and API base path. Before
validation or game launch, it finds the saved profile by its exact display name
and compares the current endpoint with that binding. If the profile was removed,
the endpoint changed, or the binding is missing, it clears the retained login
without sending the token. The next login uses the currently selected profile.
These local invalidations are separate from network failures.

Login captures the selected profile and the endpoint receiving the credentials.
If the selection, profile name, or authentication endpoint changes while login
is pending, the launcher discards a successful response and requires another
login. It never rebinds a session to a later selection.

## Logout

`DELETE /api/v1/sessions/<session_id>`

Logout sends no body. Any 2xx response succeeds, although revocation is not
guaranteed. Local logout completes even if the endpoint fails, is unavailable,
or is unimplemented.

Before requesting revocation, the backend checks the issuing endpoint binding.
If the binding is missing or mismatched, it skips the request and completes
local logout.

## Launcher rate-limit behavior

The server sets rate-limit policy. When login or registration returns 429, the
launcher honors `Retry-After`, shows the remaining wait, and re-enables
submission at zero. It never resubmits automatically.
