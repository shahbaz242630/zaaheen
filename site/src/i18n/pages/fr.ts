// French page titles and descriptions (src/i18n/pages.ts), keyed by English path.
// Prices and the trial length are typed here: importing PRICES or TRIAL from
// data/site.ts would be an import cycle (site.ts reads this file as it loads).
import type { PageMeta } from '../pages';

export default {
  '/': {
    title: 'Zaaheen : une mémoire privée pour Claude, ChatGPT et Cursor',
    description:
      'Zaaheen donne à Claude, Cursor, ChatGPT et vos autres IA une mémoire partagée, chiffrée sur votre ordinateur. Windows et Mac, 30 jours gratuits.',
  },
  '/products/': {
    title: 'Produits : une mémoire privée pour vos IA · Zaaheen',
    description:
      'Les produits de Zaaheen, à commencer par une mémoire privée et chiffrée que vos assistants d’IA partagent, conservée sur votre propre ordinateur.',
  },
  '/pricing/': {
    title: 'Tarifs : 5 USD par mois après 30 jours gratuits · Zaaheen',
    description:
      'Tarifs de Zaaheen : 30 jours gratuits sans carte, puis 5 USD par mois ou 48 USD par an. Comparé aux autres mémoires pour l’IA, ChatGPT et Claude.',
  },
  '/docs/': {
    crumb: 'Documentation',
    title: 'Aide et guides de l’application de mémoire · Zaaheen',
    description:
      'Les guides des produits Zaaheen : connecter Claude, Cursor et d’autres applications d’IA, protéger vos données et résoudre les problèmes courants.',
  },
  '/docs/getting-started/': {
    crumb: 'Bien démarrer',
    heading: 'Comment installer et configurer Zaaheen',
    title: 'Installer et configurer Zaaheen sur Windows ou Mac',
    description:
      'Installez Zaaheen sur Windows ou Mac, créez votre compte, choisissez où garder vos souvenirs et connectez votre première IA. En trois minutes.',
  },
  '/docs/subscribe/': {
    crumb: 'Comment s’abonner',
    heading: 'Comment s’abonner à Zaaheen',
    title: 'S’abonner à Zaaheen : 5 USD par mois ou 48 USD par an',
    description:
      'Créez votre compte, téléchargez Zaaheen, profitez de 30 jours gratuits, puis abonnez-vous pour 5 USD par mois ou 48 USD par an.',
  },
  '/docs/connect-claude/': {
    crumb: 'Connecter Claude Desktop',
    heading: 'Comment connecter Claude Desktop à Zaaheen',
    title: 'Connecter Claude Desktop à Zaaheen : une mémoire d’IA partagée',
    description:
      'Donnez à l’application Claude une mémoire privée partagée avec vos autres applications d’IA : un clic dans Zaaheen, puis Install dans Claude.',
  },
  '/docs/connect-claude-code/': {
    crumb: 'Connecter Claude Code',
    heading: 'Comment connecter Claude Code à Zaaheen',
    title: 'Connecter Claude Code à Zaaheen : une mémoire entre sessions',
    description:
      'Donnez à Claude Code une mémoire partagée avec vos autres IA : une commande dans un terminal, et Zaaheen est là dans tous vos projets.',
  },
  '/docs/connect-cursor/': {
    crumb: 'Connecter Cursor',
    heading: 'Comment connecter Cursor à Zaaheen',
    title: 'Connecter Cursor à Zaaheen : une mémoire d’une session à l’autre',
    description:
      'Donnez à Cursor une mémoire partagée avec vos autres IA : un clic dans Zaaheen, puis Install dans Cursor. Vérification et réglage à la main.',
  },
  '/docs/connect-chatgpt/': {
    crumb: 'Connecter ChatGPT',
    heading: 'Comment connecter ChatGPT à Zaaheen',
    title: 'Connecter ChatGPT à Zaaheen : une mémoire d’IA partagée',
    description:
      'Connectez l’application ChatGPT de votre ordinateur à Zaaheen (Settings, Plugins, MCP) et utilisez-la dans un chat Work ou Codex.',
  },
  '/docs/connect-codex/': {
    crumb: 'Connecter Codex',
    heading: 'Comment connecter Codex à Zaaheen',
    title: 'Connecter Codex à Zaaheen : une mémoire d’IA partagée',
    description:
      'Donnez à Codex d’OpenAI une mémoire que vos autres applications d’IA partagent : ajoutez le réglage de Zaaheen à config.toml, redémarrez Codex et vérifiez /mcp.',
  },
  '/docs/connect-antigravity/': {
    crumb: 'Connecter Antigravity',
    heading: 'Comment connecter Antigravity à Zaaheen',
    title: 'Connecter Antigravity à Zaaheen : une mémoire d’IA partagée',
    description:
      'Donnez à Antigravity de Google une mémoire partagée avec vos autres IA : collez le réglage de Zaaheen dans mcp_config.json, redémarrez, vérifiez.',
  },
  '/docs/connect-other-apps/': {
    crumb: 'Connecter d’autres applications d’IA',
    heading: 'Comment connecter d’autres applications d’IA à Zaaheen',
    title: 'Connecter d’autres applications d’IA à Zaaheen avec MCP',
    description:
      'Zaaheen fonctionne avec toute application d’IA compatible MCP : les guides par application, le réglage pour les autres et comment vérifier.',
  },
  '/docs/troubleshooting/': {
    crumb: 'Dépannage',
    heading: 'Résoudre les problèmes courants avec Zaaheen',
    title: 'Dépannage de Zaaheen : résoudre les problèmes courants',
    description:
      'Que faire quand une application d’IA n’utilise pas votre mémoire Zaaheen ou n’apparaît pas comme connectée, et comment nous envoyer un journal.',
  },
  '/docs/contact/': {
    crumb: 'Nous contacter',
    heading: 'Contacter Zaaheen',
    title: 'Nous contacter · Zaaheen',
    description:
      'Où trouver des réponses sur l’application de mémoire Zaaheen, comment nous écrire par e-mail et quoi joindre pour que nous puissions vous aider du premier coup.',
  },
} satisfies Record<string, PageMeta>;
