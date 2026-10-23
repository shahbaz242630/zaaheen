// pt Learn article titles, descriptions, headings, crumbs and summaries (src/i18n/pages.ts), keyed by English path.
import type { PageMeta } from '../pages';

export default {
  '/learn/': {
    crumb: 'Aprenda',
    title: 'Aprenda: memória de IA no Claude, ChatGPT e Cursor · Zaaheen',
    description:
      'Respostas simples sobre memória de IA: como o Claude, o ChatGPT, o Claude Code e o Cursor lembram de você, e como manter uma única memória em todos eles.',
  },
  '/learn/share-memory-between-chatgpt-and-claude/': {
    crumb: 'Compartilhar memória entre ChatGPT e Claude',
    heading: 'Como compartilhar memória entre o ChatGPT e o Claude',
    title: 'Como compartilhar memória entre o ChatGPT e o Claude',
    description:
      'O ChatGPT e o Claude têm cada um a sua memória e não leem a do outro. Três formas de levar o que eles sabem sobre você de um app para o outro.',
    summary:
      'Um não consegue ler a memória do outro. Copie de um para o outro, use a importação do Claude ou dê aos dois apps uma memória compartilhada.',
  },
  '/learn/does-claude-code-remember-between-sessions/': {
    crumb: 'O Claude Code lembra entre sessões?',
    heading: 'O Claude Code lembra entre uma sessão e outra?',
    title: 'O Claude Code lembra entre uma sessão e outra? · Zaaheen',
    description:
      'Sim, dentro de um projeto, em um computador. O que o CLAUDE.md e a auto memory do Claude Code guardam, o que carrega em cada sessão e o que não levam junto.',
    summary:
      'Sim, dentro de um projeto, em um computador. O que o CLAUDE.md e a auto memory guardam, o que carrega em cada sessão e o que fica para trás.',
  },
  '/learn/one-memory-across-claude-code-cursor-and-codex/': {
    crumb: 'Uma memória para Claude Code, Cursor e Codex',
    heading: 'Como manter uma única memória no Claude Code, no Cursor e no Codex',
    title: 'Uma memória para Claude Code, Cursor e Codex · Zaaheen',
    description:
      'Claude Code, Cursor e Codex têm cada um a sua memória. Compartilhe as regras do projeto com um AGENTS.md, e o que é verdade sobre você com uma memória única.',
    summary:
      'Cada um tem a sua memória. Compartilhe as regras do projeto com um AGENTS.md, e o que eles sabem sobre você com uma memória compartilhada.',
  },
  '/learn/ai-memory-tool-or-notes-file/': {
    crumb: 'Ferramenta de memória ou arquivo de notas?',
    heading: 'Você precisa de uma ferramenta de memória de IA, ou um arquivo de notas basta?',
    title: 'Memória de IA ou arquivo de notas? Uma resposta honesta · Zaaheen',
    description:
      'Para um projeto em uma ferramenta de programação, o CLAUDE.md ou o AGENTS.md costuma bastar. Quando um arquivo de notas deixa de dar conta, e como usar os dois.',
    summary:
      'Muitas vezes basta para um projeto em uma ferramenta. Onde um arquivo não dá conta, e por que você provavelmente vai querer os dois.',
  },
  '/learn/switch-from-chatgpt-to-claude/': {
    crumb: 'Trocar o ChatGPT pelo Claude',
    heading: 'Como trocar o ChatGPT pelo Claude sem começar do zero',
    title: 'Trocar o ChatGPT pelo Claude sem começar do zero · Zaaheen',
    description:
      'Leve a memória, as instruções personalizadas e as conversas antigas do ChatGPT para o Claude: as etapas, quanto tempo cada uma leva e como não repetir isso.',
    summary:
      'Leve sua memória, suas instruções e suas conversas antigas do ChatGPT para o Claude, quanto tempo cada etapa leva e como nunca mais mudar tudo de novo.',
  },
  '/learn/ai-memory-apps-compared/': {
    crumb: 'Apps de memória de IA comparados',
    heading: 'Apps de memória de IA comparados: quais funcionam no ChatGPT, no Claude e no Cursor',
    title: 'Apps de memória de IA: Supermemory, Mem0 e mais · Zaaheen',
    description:
      'Apps que dão ao ChatGPT, Claude e Cursor uma memória compartilhada, comparados em onde as memórias ficam, configuração e preço, segundo o site de cada empresa.',
    summary:
      'Cinco apps que dão aos seus apps de IA uma memória compartilhada, lado a lado: onde as memórias ficam, quais apps alcançam, configuração e preço.',
  },
  '/learn/chatgpt-memory-vs-claude-memory/': {
    crumb: 'Memória do ChatGPT vs memória do Claude',
    heading: 'Memória do ChatGPT vs memória do Claude: como cada um lembra de você',
    title: 'Memória do ChatGPT vs memória do Claude, lado a lado · Zaaheen',
    description:
      'Como o ChatGPT e o Claude lembram de você, o que você pode ver e mudar, como tratam detalhes sensíveis e o que fazer se você usa os dois.',
    summary:
      'O que cada um usa, o que você pode ver e mudar, detalhes sensíveis e o que fazer se você usa os dois.',
  },
  '/learn/mem0-alternative/': {
    crumb: 'Alternativa ao Mem0',
    heading: 'Uma alternativa ao Mem0 para quem só usa apps de IA',
    title: 'Alternativa ao Mem0: uma memória privada, sem programar · Zaaheen',
    description:
      'O Mem0 é memória para desenvolvedores, guardada na sua conta Mem0. Como o Zaaheen é diferente, onde o Mem0 é a melhor escolha e onde o Zaaheen fica devendo.',
    summary:
      'O Mem0 é feito para desenvolvedores e guarda as memórias na sua conta Mem0. Quando um app no seu próprio computador combina mais com você.',
  },
  '/learn/supermemory-alternative/': {
    crumb: 'Alternativa ao Supermemory',
    heading: 'Uma alternativa ao Supermemory que se instala como um app comum',
    title: 'Alternativa ao Supermemory: criptografada, sem terminal · Zaaheen',
    description:
      'O Supermemory é um motor de memória feito sobretudo para desenvolvedores. Como o Zaaheen difere, onde o Supermemory é melhor e onde o Zaaheen fica devendo.',
    summary:
      'O Supermemory é um motor de memória principalmente para desenvolvedores. Quando um app criptografado, sem terminal, combina mais com você.',
  },
  '/learn/memoryplugin-alternative/': {
    crumb: 'Alternativa ao MemoryPlugin',
    heading: 'Uma alternativa ao MemoryPlugin que guarda suas memórias no seu computador',
    title: 'Alternativa ao MemoryPlugin: memórias no seu computador · Zaaheen',
    description:
      'O MemoryPlugin mantém uma memória para mais de 21 ferramentas de IA no serviço dele. Como o Zaaheen se compara em armazenamento, apps e preço, e qual é melhor.',
    summary:
      'O MemoryPlugin alcança mais de 21 ferramentas de IA a partir do serviço dele. Quando guardar as memórias no seu computador importa mais.',
  },
  '/learn/best-memory-mcp-server-for-claude-code/': {
    crumb: 'Servidores de memória para o Claude Code',
    heading: 'Qual servidor MCP de memória usar com o Claude Code?',
    title: 'Melhor MCP de memória para o Claude Code? As opções · Zaaheen',
    description:
      'O servidor de referência da Anthropic, mcp-memory-service, Basic Memory, Mem0 e Zaaheen comparados no Claude Code: configuração, onde ficam as memórias, custo.',
    summary:
      'Os servidores grátis, Basic Memory, Mem0 e Zaaheen para o Claude Code: configuração, onde as memórias ficam e quanto custa.',
  },
  '/learn/obsidian-vs-ai-memory-app/': {
    crumb: 'Obsidian vs um app de memória',
    heading: 'Obsidian ou um app de memória para a sua IA?',
    title: 'Obsidian como memória de IA vs um app de memória · Zaaheen',
    description:
      'No Obsidian você escreve as notas; um app de memória é escrito pelos seus apps de IA enquanto você conversa. No que cada um é bom, o custo e o uso dos dois.',
    summary:
      'Notas que você escreve versus uma memória que seus apps de IA escrevem enquanto você conversa. No que cada um é bom, o custo e como usar os dois.',
  },
} satisfies Record<string, PageMeta>;
