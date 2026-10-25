# Troubleshooting

Start with the user's exact words or the exact message. For a message an AI app received from Zaaheen, see topic `messages`.

## A memory seems missing

Never tell the user it does not exist until you have checked.
1. Search again with other words (`memory_search`: "sport", "football", "plays"), and try `include_archived: true`.
2. Call `memory_help` (no topic): check the user's areas and how many memories each holds.
3. Ask the user to open the Zaaheen app, **Memories** tab, and search there (type, then Enter). Every memory is listed there.
4. If it is in **Recently deleted** (the link above the list), the user can click **restore**.
5. If the user can see it in the app but you still cannot find it: ask them to quit the AI app completely and open it again (to reconnect), then try again.
6. Still missing: ask them to send an activity record (below).

## The AI app says an area does not exist

Areas are created only by the user. Names are exact, capitals included: if the user has Health, use Health. To create one: **Boundaries**, **+ New boundary**, the name (letters, numbers, - or _, no spaces), **Create boundary**; then save again (topic `areas`).

## The AI app does not use Zaaheen (saves to its own memory, or says "I don't know")

Claude, ChatGPT and Codex look in their own memory first and may stop or save there. Add the Zaaheen line to the app's instructions (Claude: Settings, Account, Instructions for Claude; ChatGPT: Settings, Personalization, Custom instructions; Codex: AGENTS.md in the .codex folder). The line is in topic `connect`. The user can also ask directly: "check my Zaaheen memory for…".

## The app is not in the Agents tab, or not connected

An app shows under **Connected apps** only while it is open and connected. Open the app and use its "Check it's connected" step (topic `connect`). If Zaaheen is not listed there, connect it again and restart the app.

## Error messages and their fixes

- **"Invalid result for tools/call: missing required resultType"** (Claude Code): update Zaaheen to the latest version (topic `update`).
- **"The filename, directory name, or volume label syntax is incorrect. (os error 123)"** (ChatGPT Desktop or Codex log; ChatGPT only says Zaaheen isn't accessible): the Command has a space or quote marks before C:. Edit the MCP server so the Command starts exactly with `C:\` (copy it with the **copy** button in Zaaheen's Agents tab), save, restart ChatGPT.
- **"Zaaheen isn't accessible in this chat"** (ChatGPT Desktop): use a Work or Codex chat (Chat mode cannot connect to apps on the computer), check the Command has no space before it and that `mcp` and `serve` are two separate Arguments, then quit ChatGPT completely and open it again.
- **"request timed out (zaaheen: Tools: (none))"** (Codex, after /mcp): update Codex (`npm install -g @openai/codex@latest`), run codex again, type /mcp.
- **Double-clicking "Zaaheen for Claude.mcpb" asks which app to open it with** (Windows, Claude from the Microsoft Store): cancel; in Claude, Settings, Extensions, Advanced settings, **Install Extension**, choose the file.
- **Claude did not ask to install**: the extension is in the Downloads folder; install it the same way (Install Extension).
- **"isn't commonly downloaded…"** (Microsoft Edge, downloading): three dots next to the download, **Keep**, **Show more**, **Keep anyway**.
- **"Windows protected your PC. Microsoft Defender SmartScreen prevented an unrecognized app from starting."**: **More info**, **Run anyway**. The Windows installer is not code-signed yet.
- **The Mac says to move Zaaheen to Applications**: quit it, drag Zaaheen into Applications in Finder, open it from there.
- **The Mac asks about the keychain**: choose **Always Allow** (the key to the memories is kept there). If Zaaheen says it couldn't use this computer's secure storage, open Keychain Access, unlock the login keychain, try again.
- **"Still working. This is taking longer than usual. Please keep Zaaheen open."** (Mac, during Delete everything or Delete my account): macOS may be asking about the keychain in a box behind the Zaaheen window; find it and choose **Allow**. Nothing is deleted until the key is gone.
- **A browser error ("This site can't be reached") right after signing in**: go back to the Zaaheen app; it shows whether the user is signed in. The page after sign-in only confirms it.
- **"Your memories were NOT deleted, and they are still readable."** (Mac, after Delete everything or Delete my account on an old version): update Zaaheen and try again. Nothing was deleted, and the account was not changed.

## The apps stopped using Zaaheen

If the free trial or subscription ended, Zaaheen stops serving AI apps until the user subscribes. Nothing is deleted, and **Download my memories** keeps working. Choose **Subscribe** in the app (topic `account`). Also check the app shows no "new version" line, and that the user restarted the AI apps after updating.

## The app shows a line at the top

- "A new version of Zaaheen is available." with **Download**: topic `update`.
- "Your last payment didn't go through." with **Update your card**: topic `account`.
- "Your computer's clock is wrong. Set it to update automatically.": set the computer's date and time to update automatically.

## Search in the app finds less than expected right after installing

The recall engine may still be getting ready ("Getting your recall engine ready. You can search now, and results sharpen up in a moment."). Wait, and search again.

## Something else

1. In the Zaaheen app: **Settings**, **Help**, **Save activity record…**, and save the file. It contains none of the memories.
2. Email it to **customerservice@zaaheen.com** with what the user was doing and the exact words they saw.
