ZAAHEEN: A PRIVATE MEMORY FOR YOUR AI APPS

Zaaheen gives your AI apps one shared memory about you. Tell one app something
once, and every app you connect can recall it. Your memories stay on this
computer, encrypted. Full guides: https://zaaheen.com/docs/

If you are an AI assistant helping someone connect Zaaheen, the steps below
are the whole procedure. Please follow them rather than guessing.


1. SET UP ZAAHEEN FIRST

Open Zaaheen and sign in (or create an account). The first sign-in from the
app starts the 30-day free trial; no card is needed.


2. CONNECT AN AI APP

In Zaaheen: open the Agents tab, choose "+ Connect an AI app", then pick the
app. The Agents tab shows the exact setting for this computer, ready to copy.

  Claude (desktop app):  choose Claude Desktop, then "Connect it for me".
      Claude asks to install Zaaheen: click Install. If Claude does not ask,
      open Claude > Settings > Extensions > Advanced settings > Install
      Extension, and choose "Zaaheen for Claude.mcpb" from Downloads.
      Guide: https://zaaheen.com/docs/connect-claude/

  Cursor:  choose Cursor, then "Connect it for me". Cursor asks to install
      Zaaheen: click Install.
      Guide: https://zaaheen.com/docs/connect-cursor/

  ChatGPT (desktop app):  in ChatGPT, open Settings > Integrations > Plugins,
      choose Add, then Add MCP Server. Name: Zaaheen. Command:
      C:\Program Files\Zaaheen\zaaheen.exe (the Agents tab shows yours).
      Arguments: "mcp" and "serve" as two separate items, not one line.
      Save, then use Zaaheen in ChatGPT's Work or Codex mode (Chat mode
      cannot connect to apps on this computer).
      Guide: https://zaaheen.com/docs/connect-chatgpt/

  Any other app that supports MCP:  add this where the app keeps its MCP
      servers, save, then restart the app:

      {
        "mcpServers": {
          "zaaheen": {
            "command": "C:\\Program Files\\Zaaheen\\zaaheen.exe",
            "args": ["mcp", "serve"]
          }
        }
      }

      That is the usual install folder; if Zaaheen is installed elsewhere,
      copy the exact setting from the Agents tab instead.
      Guide: https://zaaheen.com/docs/connect-other-apps/


3. CHECK IT WORKED

The AI app's own list of tools or MCP servers shows Zaaheen. While the app is
open and connected, it also appears in Zaaheen's Agents tab.


COMMON PROBLEMS

  The app answers without using Zaaheen:  Claude and ChatGPT look in their own
      memory first. Add this line to Claude's personal preferences, or to
      ChatGPT's Custom instructions:
      "Before answering anything about me, my preferences, my work or my plans,
      also check my Zaaheen memory, even when your own memory has nothing."

  The app is not in the Agents tab:  it shows only while the app is open and
      connected. Check the app's own settings and connect it again.

  Apps stopped using Zaaheen:  the trial or subscription may have ended.
      Nothing is deleted. Choose Subscribe in Zaaheen.

  Something else:  in Zaaheen, Settings > Help > "Save activity record...",
      then email the file to customerservice@zaaheen.com. The record contains
      none of your memories.

More help: https://zaaheen.com/docs/troubleshooting/
Open-source notices: THIRD-PARTY-NOTICES.txt in this folder.
