# Installing Zaaheen

## What the user needs

- **Windows:** 64-bit Windows.
- **Mac:** a Mac with an Apple chip (M1 or newer).
- The Getting started page on zaaheen.com lists the systems each version was tested on.
- Not available for Linux or phones yet.
- An internet connection for signing in and for the first start (Zaaheen downloads what it needs to search and tidy memories, then works offline).

## Download

1. Go to zaaheen.com and choose the download for Windows (a file ending in `.msi`) or Mac (a file ending in `.dmg`). The Getting started page has both, with each file's fingerprint (SHA-256).
2. Optional check: compare the downloaded file's fingerprint with the one on the Getting started page. On Windows, in PowerShell: `Get-FileHash "$HOME\Downloads\<file name>"`. On a Mac, in Terminal: `shasum -a 256 ~/Downloads/<file name>`. If they differ, delete the file and download it again.

## Install on Windows

1. Open the downloaded installer. Up to three questions may appear, and each is expected:
   - **Microsoft Edge says the file "isn't commonly downloaded".** The installer is new, so Edge has not seen it often. Click the three dots next to the download, then **Keep**, **Show more**, **Keep anyway**. Chrome does not ask.
   - **Windows says the publisher is unknown** ("Windows protected your PC"). The Windows installer is not code-signed yet. Click **More info**, then **Run anyway**.
   - **Windows asks for administrator permission.** Zaaheen installs for everyone on the computer, so Windows asks once. Click Yes. Zaaheen does not need administrator rights to run afterwards.
2. Follow the installer to the end, then open Zaaheen from the Start menu.

## Install on a Mac

1. Open the downloaded disk image.
2. Drag **Zaaheen** into the **Applications** folder.
3. Open Zaaheen from Applications. It only runs from there; if the Mac says to move it to Applications, quit it, drag it into Applications in Finder, and open it from there.
4. The Mac asks whether to open an app downloaded from the internet, naming the developer. The Mac app is signed and checked by Apple. Click **Open**.
5. The Mac may ask whether Zaaheen can use the keychain (where the key to the memories is kept). Click **Always Allow**.

## Set up (first start)

Zaaheen opens on a welcome screen: "One memory for all your AI apps, kept safe on your computer." Click **Begin set up**. Setup takes about three minutes, one screen at a time:

1. **Account.** "Create your Zaaheen account, or sign in." Click **Create an account** or **Sign in**. This opens the web browser. The user can use their email (a code is emailed to confirm) or Google. The free trial starts the first time they sign in from the app; no card is needed. Signing in never shares the memories. After finishing in the browser, go back to the Zaaheen app (if the browser shows an error page after signing in, it does not matter; the app shows whether sign-in worked).
2. **Where to keep the memories.** "Where should Zaaheen keep your memories?" The usual place suits most people: click **Continue**. To use another folder or drive, click **Choose another folder…**, then **Move them here** (Zaaheen closes, moves them and opens again by itself, then setup carries on). It can be changed later in Settings.
3. **First AI app.** "Connect your first AI app." Pick the app on the left and follow its numbered steps (topic `connect`), then **Continue**; or click **I'll connect one later**.
4. **First memory.** "What should your AI apps know about you?" Type one thing about the user (for example "I prefer short, direct answers."), choose the area, and click **Keep this memory**; or **Skip for now**.
5. **Tidying up.** "Keep your memory sharp on its own." Leave **Yes, tidy automatically** (every day, at 03:00 unless another time is chosen) or choose **Not now**. Then click **Finish setup**. On a Mac, macOS may announce the schedule as a background item; that is expected.

While setup runs, Zaaheen downloads, in the background, what it needs to search memories well (the "recall engine"). Setup waits for it before finishing. Until it is ready, search still works but ranks less sharply; the Memories tab says "Getting your recall engine ready". The tidy-up uses its own on-device engine, a one-time large download made when tidying is turned on; the Consolidation tab shows its progress.

Then the Memories tab opens.

## After installing

- Connect each AI app the user uses, once, from the **Agents** tab (topic `connect`).
- Restart each AI app after connecting it.

## Uninstalling

See topic `uninstall`. In short: uninstalling removes the app, not the memories, and does not cancel a subscription.
