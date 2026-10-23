// fr Learn article titles, descriptions, headings, crumbs and summaries (src/i18n/pages.ts), keyed by English path.
import type { PageMeta } from '../pages';

export default {
  '/learn/': {
    crumb: 'Apprendre',
    title: 'La mémoire de l’IA dans Claude, ChatGPT et Cursor · Zaaheen',
    description:
      'Des réponses simples sur la mémoire de l’IA : comment Claude, ChatGPT, Claude Code et Cursor se souviennent de vous, et comment garder une mémoire pour tous.',
  },
  '/learn/share-memory-between-chatgpt-and-claude/': {
    crumb: 'Partager la mémoire entre ChatGPT et Claude',
    heading: 'Comment partager la mémoire entre ChatGPT et Claude',
    title: 'Comment partager la mémoire entre ChatGPT et Claude',
    description:
      'ChatGPT et Claude ont chacun leur mémoire et ne peuvent pas lire celle de l’autre. Trois façons de transférer ce qu’ils savent de vous de l’un à l’autre.',
    summary:
      'Ils ne peuvent pas lire la mémoire l’un de l’autre. Recopiez-la, utilisez l’import de Claude, ou donnez aux deux applications une mémoire partagée.',
  },
  '/learn/does-claude-code-remember-between-sessions/': {
    crumb: 'Claude Code se souvient-il d’une session à l’autre ?',
    heading: 'Claude Code se souvient-il d’une session à l’autre ?',
    title: 'Claude Code se souvient-il d’une session à l’autre ? · Zaaheen',
    description:
      'Oui, dans un projet sur un ordinateur. Ce que gardent CLAUDE.md et l’auto memory de Claude Code, ce qui se charge à chaque session, et ce qui ne suit pas.',
    summary:
      'Oui, dans un même projet sur un même ordinateur. Ce que gardent CLAUDE.md et l’auto memory, ce qui se charge à chaque session, et ce qu’ils laissent de côté.',
  },
  '/learn/one-memory-across-claude-code-cursor-and-codex/': {
    crumb: 'Une mémoire pour Claude Code, Cursor et Codex',
    heading: 'Comment garder une seule mémoire pour Claude Code, Cursor et Codex',
    title: 'Une seule mémoire pour Claude Code, Cursor et Codex · Zaaheen',
    description:
      'Claude Code, Cursor et Codex ont chacun leur mémoire. Partagez les règles du projet avec un AGENTS.md, et ce qui est vrai sur vous avec une mémoire partagée.',
    summary:
      'Chacun a sa propre mémoire. Partagez les règles du projet avec un seul AGENTS.md, et ce qu’ils savent de vous avec une mémoire partagée.',
  },
  '/learn/ai-memory-tool-or-notes-file/': {
    crumb: 'Outil de mémoire ou fichier de notes ?',
    heading: 'Faut-il un outil de mémoire pour l’IA, ou un fichier de notes suffit-il ?',
    title: 'Outil de mémoire ou fichier de notes ? Réponse honnête · Zaaheen',
    description:
      'Pour un projet dans un outil de code, CLAUDE.md ou AGENTS.md suffit souvent. Quand un fichier de notes ne suffit plus, et comment utiliser les deux ensemble.',
    summary:
      'Souvent suffisant pour un projet dans un seul outil. Là où un fichier ne suffit plus, et pourquoi vous voudrez sans doute les deux.',
  },
  '/learn/switch-from-chatgpt-to-claude/': {
    crumb: 'Passer de ChatGPT à Claude',
    heading: 'Comment passer de ChatGPT à Claude sans repartir de zéro',
    title: 'Passer de ChatGPT à Claude sans repartir de zéro · Zaaheen',
    description:
      'Transférez mémoire, instructions et anciennes conversations de ChatGPT vers Claude : les étapes, leur durée, et comment ne plus jamais déménager.',
    summary:
      'Transférez votre mémoire, vos instructions et vos anciennes conversations de ChatGPT vers Claude, la durée de chaque étape, et comment ne plus jamais déménager.',
  },
  '/learn/ai-memory-apps-compared/': {
    crumb: 'Applications de mémoire d’IA comparées',
    heading: 'Applications de mémoire d’IA comparées : lesquelles fonctionnent avec ChatGPT, Claude et Cursor',
    title: 'Mémoire d’IA : Supermemory, Mem0 et d’autres comparés · Zaaheen',
    description:
      'Des applications qui donnent à ChatGPT, Claude et Cursor une mémoire partagée, comparées sur le stockage, la mise en place et le prix, d’après leur propre site.',
    summary:
      'Cinq applications qui donnent à vos applications d’IA une mémoire partagée, côte à côte : où sont conservés les souvenirs, quelles applications, mise en place et prix.',
  },
  '/learn/chatgpt-memory-vs-claude-memory/': {
    crumb: 'Mémoire de ChatGPT ou mémoire de Claude',
    heading: 'Mémoire de ChatGPT ou mémoire de Claude : comment chacun se souvient de vous',
    title: 'Mémoire de ChatGPT et mémoire de Claude, côte à côte · Zaaheen',
    description:
      'Comment ChatGPT et Claude se souviennent de vous, ce que vous pouvez voir et modifier, les informations sensibles, et que faire si vous utilisez les deux.',
    summary:
      'Dans quoi chacun puise, ce que vous pouvez voir et modifier, les informations sensibles, et que faire si vous utilisez les deux.',
  },
  '/learn/mem0-alternative/': {
    crumb: 'Alternative à Mem0',
    heading: 'Une alternative à Mem0 pour les personnes qui utilisent simplement des applications d’IA',
    title: 'Alternative à Mem0 : une mémoire privée, sans coder · Zaaheen',
    description:
      'Mem0 est une mémoire pour développeurs, conservée dans votre compte Mem0. Ce qui distingue Zaaheen, quand Mem0 est le meilleur choix, et les limites de Zaaheen.',
    summary:
      'Mem0 est conçu pour les développeurs et garde les souvenirs dans votre compte Mem0. Quand une application sur votre propre ordinateur vous convient mieux.',
  },
  '/learn/supermemory-alternative/': {
    crumb: 'Alternative à Supermemory',
    heading: 'Une alternative à Supermemory qui s’installe comme une application normale',
    title: 'Alternative à Supermemory : chiffrée, sans terminal · Zaaheen',
    description:
      'Supermemory est un moteur de mémoire surtout pour développeurs. Ce qui distingue Zaaheen, quand Supermemory est le meilleur choix, et les limites de Zaaheen.',
    summary:
      'Supermemory est un moteur de mémoire surtout pour développeurs. Quand une application chiffrée, sans terminal, vous convient mieux.',
  },
  '/learn/memoryplugin-alternative/': {
    crumb: 'Alternative à MemoryPlugin',
    heading: 'Une alternative à MemoryPlugin qui garde vos souvenirs sur votre ordinateur',
    title: 'Alternative à MemoryPlugin : vos souvenirs chez vous · Zaaheen',
    description:
      'MemoryPlugin garde une mémoire pour plus de 21 outils d’IA sur son service. Comment Zaaheen se compare sur le stockage, les applications et le prix.',
    summary:
      'MemoryPlugin atteint plus de 21 outils d’IA depuis son propre service. Quand garder vos souvenirs sur votre ordinateur compte davantage.',
  },
  '/learn/best-memory-mcp-server-for-claude-code/': {
    crumb: 'Serveurs de mémoire pour Claude Code',
    heading: 'Quel serveur MCP de mémoire utiliser avec Claude Code ?',
    title: 'Quel serveur MCP de mémoire pour Claude Code ? · Zaaheen',
    description:
      'Le serveur de référence d’Anthropic, mcp-memory-service, Basic Memory, Mem0 et Zaaheen comparés pour Claude Code : mise en place, stockage des souvenirs, coût.',
    summary:
      'Les serveurs gratuits, Basic Memory, Mem0 et Zaaheen pour Claude Code : mise en place, où vivent les souvenirs, et coût.',
  },
  '/learn/obsidian-vs-ai-memory-app/': {
    crumb: 'Obsidian ou une application de mémoire',
    heading: 'Obsidian ou une application de mémoire pour votre IA ?',
    title: 'Obsidian ou une application de mémoire pour l’IA · Zaaheen',
    description:
      'Obsidian, ce sont des notes que vous écrivez ; une application de mémoire, vos applications d’IA l’écrivent en parlant. Les points forts, le coût, et les deux.',
    summary:
      'Des notes que vous écrivez, ou une mémoire que vos applications d’IA écrivent pendant que vous parlez. Les points forts de chacun, le coût, et utiliser les deux.',
  },
} satisfies Record<string, PageMeta>;
