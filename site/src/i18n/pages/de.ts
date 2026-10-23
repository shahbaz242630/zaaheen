// German page titles and descriptions (src/i18n/pages.ts), keyed by English path.
// Prices and the trial length are written out here: this file is loaded while
// src/data/site.ts is still being read, so it cannot import PRICES or TRIAL.
// They match PRICES (5 and 48 USD) and TRIAL (30 days).
import type { PageMeta } from '../pages';

export default {
  '/': {
    title: 'Zaaheen: ein privates Gedächtnis für Claude, ChatGPT und Cursor',
    description:
      'Zaaheen gibt Claude, Cursor, ChatGPT und anderen KI-Apps ein gemeinsames Gedächtnis, verschlüsselt auf deinem Computer. Für Windows und Mac, 30 Tage gratis.',
  },
  '/products/': {
    title: 'Produkte: ein privates Gedächtnis für deine KI-Apps · Zaaheen',
    description:
      'Die Produkte von Zaaheen, angefangen mit einem privaten, verschlüsselten Gedächtnis, das deine KI-Assistenten auf deinem eigenen Computer gemeinsam nutzen.',
  },
  '/pricing/': {
    title: 'Preise: 5 USD pro Monat nach 30 Tagen kostenlos · Zaaheen',
    description:
      'Zaaheen Preise: 30 Tage gratis ohne Karte, dann 5 USD pro Monat oder 48 USD pro Jahr. Im Vergleich mit anderen KI-Gedächtnis-Tools, ChatGPT und Claude.',
  },
  '/docs/': {
    crumb: 'Dokumente',
    title: 'Hilfe und Anleitungen für die Gedächtnis-App · Zaaheen',
    description:
      'Anleitungen für Zaaheen: Claude, Cursor und andere KI-Apps verbinden, deine Daten schützen und häufige Probleme beheben, Schritt für Schritt erklärt.',
  },
  '/docs/getting-started/': {
    crumb: 'Erste Schritte',
    heading: 'So installierst du Zaaheen und richtest es ein',
    title: 'Zaaheen auf Windows oder Mac installieren und einrichten',
    description:
      'Installiere Zaaheen unter Windows oder auf dem Mac, erstelle dein Konto, wähle den Ort deiner Erinnerungen, verbinde deine erste KI-App. In etwa drei Minuten.',
  },
  '/docs/subscribe/': {
    crumb: 'So schließt du ein Abo ab',
    heading: 'So schließt du ein Abo für Zaaheen ab',
    title: 'Zaaheen abonnieren: 5 USD pro Monat oder 48 USD pro Jahr',
    description:
      'Konto erstellen, Zaaheen herunterladen, 30 Tage kostenlos testen. Dann das Abo für 5 USD pro Monat oder 48 USD pro Jahr, in der App oder auf der Account-Seite.',
  },
  '/docs/connect-claude/': {
    crumb: 'Claude Desktop verbinden',
    heading: 'So verbindest du Claude Desktop mit Zaaheen',
    title: 'Claude Desktop mit Zaaheen verbinden: gemeinsames KI-Gedächtnis',
    description:
      'Gib der Claude-App auf deinem Computer ein privates Gedächtnis, das sie mit deinen anderen KI-Apps teilt: ein Klick in Zaaheen, dann Install in Claude.',
  },
  '/docs/connect-claude-code/': {
    crumb: 'Claude Code verbinden',
    heading: 'So verbindest du Claude Code mit Zaaheen',
    title: 'Claude Code mit Zaaheen verbinden: Gedächtnis für jede Sitzung',
    description:
      'Gib Claude Code ein Gedächtnis, das deine anderen KI-Apps teilen: Führe einen Befehl im Terminal aus, und Zaaheen ist in jedem Claude-Code-Projekt verfügbar.',
  },
  '/docs/connect-cursor/': {
    crumb: 'Cursor verbinden',
    heading: 'So verbindest du Cursor mit Zaaheen',
    title: 'Cursor mit Zaaheen verbinden: Gedächtnis über Sitzungen hinweg',
    description:
      'Gib Cursor ein Gedächtnis, das deine anderen KI-Apps teilen: ein Klick in Zaaheen, dann Install in Cursor. So prüfst du es, und die Einstellung von Hand.',
  },
  '/docs/connect-chatgpt/': {
    crumb: 'ChatGPT verbinden',
    heading: 'So verbindest du ChatGPT mit Zaaheen',
    title: 'ChatGPT mit Zaaheen verbinden: gemeinsames KI-Gedächtnis',
    description:
      'Verbinde die ChatGPT-App für deinen Computer über Settings, Plugins, MCP mit Zaaheen, nutze es in einem Work- oder Codex-Chat und lass ChatGPT dort nachsehen.',
  },
  '/docs/connect-codex/': {
    crumb: 'Codex verbinden',
    heading: 'So verbindest du Codex mit Zaaheen',
    title: 'Codex mit Zaaheen verbinden: gemeinsames KI-Gedächtnis',
    description:
      'Gib Codex von OpenAI ein Gedächtnis, das deine anderen KI-Apps teilen: Einstellung von Zaaheen in config.toml eintragen, Codex neu starten und mit /mcp prüfen.',
  },
  '/docs/connect-antigravity/': {
    crumb: 'Antigravity verbinden',
    heading: 'So verbindest du Antigravity mit Zaaheen',
    title: 'Antigravity mit Zaaheen verbinden: gemeinsames KI-Gedächtnis',
    description:
      'Gib Antigravity von Google ein Gedächtnis, das deine anderen KI-Apps teilen: Zaaheens Einstellung in mcp_config.json einfügen, neu starten, Verbindung prüfen.',
  },
  '/docs/connect-other-apps/': {
    crumb: 'Andere KI-Apps verbinden',
    heading: 'So verbindest du andere KI-Apps mit Zaaheen',
    title: 'Andere KI-Apps per MCP mit Zaaheen verbinden',
    description:
      'Zaaheen funktioniert mit jeder KI-App, die MCP unterstützt. Welche Apps eine eigene Anleitung haben, die Einstellung für alle anderen, und wie du prüfst.',
  },
  '/docs/troubleshooting/': {
    crumb: 'Fehlerbehebung',
    heading: 'Häufige Probleme mit Zaaheen beheben',
    title: 'Zaaheen Fehlerbehebung: häufige Probleme lösen',
    description:
      'Was tun, wenn eine KI-App dein Zaaheen-Gedächtnis nicht nutzt, nicht als verbunden erscheint oder ChatGPT es nicht sieht, und wie du uns ein Protokoll schickst.',
  },
  '/docs/contact/': {
    crumb: 'Kontakt',
    heading: 'Zaaheen kontaktieren',
    title: 'Kontakt · Zaaheen',
    description:
      'Wo du Antworten zur Gedächtnis-App Zaaheen findest, wie du uns per E-Mail erreichst und was du mitschicken solltest, damit wir gleich beim ersten Mal helfen.',
  },
} satisfies Record<string, PageMeta>;
