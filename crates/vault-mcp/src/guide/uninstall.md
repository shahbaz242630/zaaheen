# Uninstalling Zaaheen

## Before uninstalling (choose what should happen to the memories)

Uninstalling removes the app, **not the memories**: they stay on the computer, encrypted, and come back if Zaaheen is installed again later. Uninstalling also does **not** cancel a subscription.

- **To keep a readable copy:** **Settings**, **Your memories**, **Download my memories**. It saves everything as one file, wherever the user chooses.
- **To erase the memories for good:** **Settings**, **Your memories**, **Delete everything**, type **DELETE**, then **Permanently delete everything**. It destroys the encryption key first, then the memory files, so nothing left on the disk or in a backup can be unlocked again. It also removes the scheduled tidy-up. Copies saved with Download my memories are readable files and are not affected: the user deletes those themselves.
- **To close the account too:** **Settings**, **Account**, **Delete my account** (topic `account`). It deletes the memories on this computer and cancels the subscription.
- **To stop paying but keep the account:** **Settings**, **Account**, **Cancel subscription**.

## Uninstall on Windows

1. Close the Zaaheen window, and the AI apps that use Zaaheen.
2. Open Windows **Settings**, **Apps**, **Installed apps**.
3. Find **Zaaheen**, click the three dots, then **Uninstall**, and confirm.

The Windows uninstaller also removes Zaaheen's scheduled nightly tidy-up and its command-line program.

## Uninstall on a Mac

1. If the memories are being kept, first turn off automatic tidying: **Consolidation** tab, untick **Keep my vault tidy automatically**. This removes the scheduled tidy-up, which dragging the app away does not. (Delete everything also removes it.)
2. Quit Zaaheen (Zaaheen menu, Quit, or Command and Q), and the AI apps that use it.
3. In Finder, open **Applications** and drag **Zaaheen** to the Trash (Bin), then empty the Trash.

## What stays on the computer afterwards

- **The memories folder** (encrypted), unless Delete everything was used. Its location is the one shown in **Settings**, **Your memories**, **Memories location** (on Windows the usual place is inside the user's AppData folder, in a folder named `com.zaaheen.app`). Without Zaaheen nobody can read it. To remove it by hand after uninstalling, delete that folder; but **Delete everything before uninstalling is the safe way**, because it also destroys the key.
- **The key to the memories**, in Windows Credential Manager or the Mac login keychain, unless Delete everything was used.
- **The AI apps' settings** still mention zaaheen. Remove Zaaheen from each AI app's MCP or extension settings (the places in topic `connect`), or the app may show that Zaaheen cannot start.

## Installing again later

Install as normal (topic `install`) and sign in with the same account. If the memories folder and key are still there, the memories come back as they were.
