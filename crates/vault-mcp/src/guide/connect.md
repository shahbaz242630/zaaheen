# Connecting AI apps

Each AI app is connected once, on each computer. All connected apps share the same memories and every area.

## Where

1. Open the Zaaheen app and click **Agents** at the top ("Your AI apps.").
2. The list on the left starts with **Connected apps** (the apps that have used the memories on this computer, with their status), then one entry per app: **Claude Desktop**, **Cursor Desktop**, **ChatGPT Desktop**, **Antigravity**, **Claude Code**, **Codex**, **Another app**.
3. Click the user's app. Its numbered steps appear on the right. "Please follow all 3 steps. If you skip one, the app may not use Zaaheen." Each setting to paste has a **copy** button: always copy from there, because it holds this computer's own command (on Windows the full path, such as `C:\Program Files\Zaaheen\zaaheen.exe`).
4. After connecting, quit the AI app completely and open it again.

An app shows under Connected apps only while it is open and connected.

## Claude Desktop (Chat and Cowork)

- **Step 1A: The quick way.** Click **Save the extension**. Zaaheen saves a small Claude extension, and Claude asks the user to install it: click **Install**. If Claude does not ask, the file "Zaaheen for Claude.mcpb" is in the Downloads folder: in Claude, open Settings, then Extensions, then Advanced settings, then **Install Extension**, and choose it. (On Windows with Claude from the Microsoft Store, double-clicking the file asks which app to open it with: cancel that and use Install Extension instead.)
- **Step 1B: Or add it yourself.** In Claude, open Settings, then Developer, then **Edit Config**. Add the setting from Zaaheen's copy box to that file, save it, then quit Claude and open it again.
- **Step 2: Check it's connected.** In Claude, Settings, Extensions: Zaaheen is listed and switched on (if added by hand, it is under Settings, Developer).
- **Step 3: Tell Claude to use Zaaheen (don't skip this).** Claude looks in its own memory first. In Claude, open Settings, then Account, and add the Zaaheen line (below) under **Instructions for Claude**.

## Cursor Desktop (and the Cursor IDE)

Connecting Cursor Desktop is enough for both: the IDE uses Cursor Desktop's settings.
- **Step 1A: The quick way.** Click **Connect it for me**. Zaaheen opens Cursor, and Cursor shows an Install box. It can take up to a minute; the box may be behind the Zaaheen window. Click **Install**.
- **Step 1B: Or add it yourself.** In Cursor Desktop, choose **Customize** in the left bar, then **MCPs**. Scroll to the bottom, choose **+ New MCP server**, paste the setting, and save.
- **Step 2: Check it's connected.** Customize, then MCPs: zaaheen is listed and connected.

## ChatGPT Desktop (Work and Codex modes)

ChatGPT's Chat mode cannot connect to apps on the computer; use a Work or Codex chat.
1. In ChatGPT Desktop, open Settings, then **Plugins** at the top, and choose **MCP**. Open the drop-down in the top right corner and choose **Add MCP server**.
2. Name: `Zaaheen`.
3. Command: paste it exactly from Zaaheen's copy box, with **no space before it** (on Windows it must start with `C:\`).
4. Arguments: type `mcp` in the first box.
5. Click **+** to add a second box, and type `serve` in it. (`mcp` and `serve` are two separate items, not one line.)
6. Click **Save**. Quit ChatGPT completely (on Windows also from its icon by the clock) and open it again.
- **Check it's connected:** Settings, Plugins, MCP: Zaaheen is listed and switched on.
- **Tell ChatGPT to use Zaaheen:** Settings, Personalization, **Custom instructions**: add the Zaaheen line.

## Antigravity (app and IDE)

1. Open its settings file. **Windows:** press Windows and R together, paste the line for the user's Antigravity, press Enter: Antigravity app `notepad %USERPROFILE%\.gemini\config\mcp_config.json`; Antigravity IDE `notepad %USERPROFILE%\.gemini\antigravity\mcp_config.json`. **Mac:** open Terminal and paste: app `mkdir -p ~/.gemini/config && touch ~/.gemini/config/mcp_config.json && open -e ~/.gemini/config/mcp_config.json`; IDE `mkdir -p ~/.gemini/antigravity && touch ~/.gemini/antigravity/mcp_config.json && open -e ~/.gemini/antigravity/mcp_config.json`.
2. Paste the setting from Zaaheen's copy box. If the file already lists other apps, add the zaaheen part next to them. Save, then close Antigravity fully and open it again.
- **Check it's connected:** Settings, Customization; under Token usage, next to MCP tools, click **Show breakdown**: Zaaheen is listed with a green circle.

## Claude Code (terminal)

1. In a terminal, run once the command from Zaaheen's copy box. It looks like `claude mcp add --transport stdio --scope user zaaheen -- "<path to zaaheen>" mcp serve`. Zaaheen is then available in every Claude Code project.
- **Check it's connected:** run `claude mcp list`: zaaheen shows as connected.

## Codex (terminal)

Already connected ChatGPT Desktop? Codex uses the same settings; only do the check.
1. Use an up-to-date Codex. Add the setting from Zaaheen's copy box to Codex's settings file `config.toml`, in the `.codex` folder in the home folder. If the file already has a `[mcp_servers.zaaheen]` section, do not add a second one. Save, then restart Codex.
- **Check it's connected:** in Codex type `/mcp` and press Enter: zaaheen is listed as connected, with its tools. If it shows no tools or timed out, update Codex (`npm install -g @openai/codex@latest`) and try again.
- **Tell Codex to use Zaaheen:** add the Zaaheen line to the file `AGENTS.md` in the `.codex` folder in the home folder (make the file if it isn't there).

## Another app

Any AI app that can connect to MCP servers on the computer: give it the setting from the **Another app** copy box. In general form:

```
{ "mcpServers": { "zaaheen": { "command": "<path to zaaheen>", "args": ["mcp", "serve"] } } }
```

Check: the app's list of tools or MCP servers shows Zaaheen.

## The Zaaheen line (makes an app use Zaaheen)

Some apps look in their own memory first and stop there, or save there. Add this line to the app's own instructions:

> When I ask you to remember something, save it in Zaaheen. Before answering anything about me, my preferences, my work or my plans, also check my Zaaheen memory, even when your own memory has nothing.

The user can also ask directly: "check my Zaaheen memory for…". Cursor and Antigravity check Zaaheen by themselves when asked about the user.

## If it does not work

See topic `troubleshooting`.
