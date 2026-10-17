---
title: Signing in
description: Get past YouTube's "confirm you're not a bot" check, and sign in with Google to like, comment, manage playlists and sync subscriptions in ytui.
---

You don't need an account to browse or watch. There are two separate kinds of sign-in, for two
different reasons.

## Sign in for playback (the bot check)

Sometimes YouTube stops serving videos to a connection it thinks is a bot. You'll see
"Sign in to confirm you're not a bot". It usually clears up on its own after a few hours. To keep watching
in the meantime, sign in:

1. In ytui, press `@` or click **Sign in** in the sidebar, then choose Sign in. Or run `ytui --signin`.
2. A browser window opens on Google's sign-in page. Sign in as you would on any website.
3. ytui takes it from there and closes the window.

The window uses a separate Chrome, Edge, Brave, Chromium or Firefox profile that belongs to ytui, never your everyday one.
ytui picks a Chromium-family browser first. To sign in with Firefox when you have both, set `YTUI_BROWSER=firefox`.
If none of those browsers is installed, set `YTUI_BROWSER` to one, or use a cookie extension instead.
To do that, open youtube.com, click the extension and press Export or Copy while ytui is waiting. Or paste the export into
ytui with `Ctrl-V`.

You can also hand cookies to ytui directly with `--cookies FILE` or `--cookies-from-browser firefox`.
Chrome and Edge usually can't be read while they're running.

The cookies stay on your computer and are only passed to yt-dlp. Treat them like a password,
because they sign in to your Google account. `ytui --signout-cookies` forgets them, and `ytui --help-cookies`
prints these steps.

## Sign in to like, comment and manage playlists

Liking, disliking, commenting and managing playlists use Google's official OAuth flow and the YouTube Data
API, with your own free Google Cloud OAuth client. ytui never sees your password. The sign-in is
saved in `~/.local/share/ytui/oauth.json`, and only you can read it.

1. In the [Google Cloud console](https://console.cloud.google.com/), create a project and turn on
   **YouTube Data API v3** (APIs & Services, then Library).
2. In Google Auth Platform, under Branding, set an app name and your email. Under Audience, choose **External** and
   add your Google account under **Test users**.
3. In Google Auth Platform, go to Clients, then Create client, then **Desktop app**, and download the JSON file.
4. Run `ytui --login --client ~/Downloads/client_secret_XXXX.json` and open the link it prints.
   Over SSH, the last page won't load, because it points at `127.0.0.1`. Copy that page's address from the
   browser and paste it into the terminal.

Once you're signed in:

- Your YouTube subscriptions are synced when ytui starts and on the Subscriptions page, and Subscribe and
  Unsubscribe change them on YouTube. Channels you subscribed to in ytui before signing in stay
  on your computer until you press "Upload … to YouTube".
- Live chat gets a "Say something…" box, or press `I`.
- `≡+ Save` on a video, or `P` on any video in a list, opens *Save to…* with your playlists. Tick or untick them,
  or create a new one. The **Playlists** page lets you play all, rename, change privacy, delete,
  and reorder or remove videos. YouTube's own Watch later isn't available through the API, so ytui's
  Watch later stays on your computer.

While your OAuth app is in *Testing*, Google ends the sign-in after 7 days. Run `ytui --login`
again, or publish the app. The free quota is 10,000 units a day, and a like or a comment costs 50.
`ytui --logout` revokes access, and `ytui --help-login` prints these steps.
