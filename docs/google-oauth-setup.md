# Google OAuth setup (one-time, ~10 min)

Google requires a human with a Google account to create these. Once the credentials exist
in the file below, everything else — the auth flow, refresh, storage — is handled by the app.

## Steps

1. **Create a project** at <https://console.cloud.google.com/> — name it `agenda`.

2. **Enable the API**: APIs & Services → Library → "Google Calendar API" → Enable.

3. **OAuth consent screen**: User type *External*. Fill app name + your email.
   - Scopes → Add → `https://www.googleapis.com/auth/calendar`
     (full read/write; `calendar.readonly` won't allow editing events)
   - **Publishing status → PUBLISH APP ("In production").**
     This matters: apps left in *Testing* expire refresh tokens after **7 days**, meaning
     you re-login weekly. Published apps issue long-lived refresh tokens.
     You will see an "unverified app" warning at first login — expected, click through it.
     Verification is only needed to remove that warning for other users; irrelevant here.

4. **Credentials** → Create credentials → OAuth client ID → Application type: **Desktop app**.
   Copy the client ID and client secret.

5. **Write them to disk yourself** — don't paste secrets into a chat:

       mkdir -p ~/.config/agenda
       $EDITOR ~/.config/agenda/oauth.toml
       chmod 600 ~/.config/agenda/oauth.toml

   Contents:

       client_id = "....apps.googleusercontent.com"
       client_secret = "GOCSPX-...."

## Notes

- For "Desktop app" clients Google treats the secret as non-confidential (it ships inside
  installed binaries by design), but keep it out of git regardless. `~/.config/agenda/` is
  outside the repo; the repo also gitignores any `oauth.toml`.
- Redirect URI is `http://127.0.0.1:<random-port>` chosen at runtime — the loopback flow
  needs no configuration in the console.
- Adding a *second* Google account later needs no new credentials: same client, new consent.
