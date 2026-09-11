---
layout: default
title: Privacy Policy
---

# Privacy policy

*Last updated: 11 September 2026*

agenda is an open-source desktop calendar application for Linux. It is software you run on
your own computer, not a service. The author operates no server, no backend and no hosted
component of any kind, and therefore never receives, stores or has any means of accessing
your data.

This policy describes what agenda does with information on your own machine.

## What agenda accesses

If you connect a Google account, agenda uses the Google Calendar API to read your calendar
data — your list of calendars and their settings, and the events on them, including titles,
descriptions, locations, times, recurrence rules and attendee information supplied by Google.

It accesses this only after you have granted permission through Google's own consent screen,
and only for the accounts you choose to connect.

## Where that information is stored

**On your computer, and nowhere else.**

- **Calendar data** is stored in a local SQLite database under your home directory, by
  default at `~/.local/share/agenda/agenda.db`.
- **OAuth tokens** (the credentials that let agenda talk to Google on your behalf) are stored
  in your desktop's Secret Service keyring — for example KDE's `ksecretd` or GNOME Keyring —
  and are protected by that keyring, not written to agenda's own files.
- **Your OAuth client credentials**, which you create yourself in the Google Cloud Console,
  are read from a configuration file you write at `~/.config/agenda/oauth.toml`.

None of these locations is synchronised, uploaded or backed up by agenda.

## What leaves your computer

Requests to Google's own servers, and nothing else:

- `accounts.google.com`, to sign in and to refresh access tokens
- `www.googleapis.com`, to read your calendar data

agenda contains **no analytics, no telemetry, no crash reporting, no advertising and no
third-party SDKs**. It makes no network request to the author or to any other party. Your
data is never sold, shared or transmitted to anyone.

Information you send to Google is handled under
[Google's Privacy Policy](https://policies.google.com/privacy).

## Google API Services User Data Policy

agenda's use of information received from Google APIs adheres to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements. Data obtained from the Google Calendar API is used
solely to display, remind you about and manage your calendar within the application on your
own device. It is not transferred to anyone, not used for advertising, and not read by any
human.

## How long it is kept

Calendar data stays in the local database until you delete it. agenda does not expire or
archive it on your behalf.

## How to remove it

- **Revoke agenda's access to your Google account** at
  [myaccount.google.com/permissions](https://myaccount.google.com/permissions).
- **Delete the local calendar data** by removing `~/.local/share/agenda/`.
- **Delete the stored tokens** by removing agenda's entries from your keyring, using your
  desktop's keyring manager, or by disconnecting the account within agenda.

Because the author holds no copy of your data, there is nothing further to request deletion
of, and no data-access request that could be answered with anything you do not already hold
yourself.

## Children

agenda is not directed at children and collects no information from anyone.

## Changes to this policy

Any change will be published on this page with an updated date above. The full history of
changes is visible in the
[repository](https://github.com/guilhermestorck/agenda/commits/main/docs/privacy-policy.md).

## Contact

Questions about this policy can be raised as an issue at
[github.com/guilhermestorck/agenda/issues](https://github.com/guilhermestorck/agenda/issues).
