// Portuguese (Brazilian) page titles and descriptions (src/i18n/pages.ts), keyed by English path.
import type { PageMeta } from '../pages';

export default {
  '/': {
    title: 'Zaaheen: uma memória privada para Claude, ChatGPT e Cursor',
    description:
      'O Zaaheen dá ao Claude, Cursor, ChatGPT e outros apps de IA uma memória compartilhada, criptografada no seu computador. Windows e Mac, 30 dias grátis.',
  },
  // The prices and trial length below are the English entries' (PLANS, TRIAL
  // in src/data/site.ts), written out: this file is loaded by site.ts, so it
  // cannot import from it.
  '/products/': {
    title: 'Produtos: uma memória privada para seus apps de IA · Zaaheen',
    description:
      'Os produtos que a Zaaheen cria, a começar por uma memória privada e criptografada que seus assistentes de IA compartilham no seu próprio computador.',
  },
  '/pricing/': {
    title: 'Preços: US$ 5 por mês depois de 30 dias grátis · Zaaheen',
    description:
      'Preços do Zaaheen: 30 dias grátis sem cartão, depois US$ 5 por mês ou US$ 48 por ano. Comparado a outras memórias de IA, ChatGPT e Claude.',
  },
  '/docs/': {
    crumb: 'Documentos',
    title: 'Ajuda e guias do app de memória · Zaaheen',
    description:
      'Guias dos produtos Zaaheen: como conectar o Claude, o Cursor e outros apps de IA, manter seus dados seguros e resolver os problemas mais comuns.',
  },
  '/docs/getting-started/': {
    crumb: 'Primeiros passos',
    heading: 'Como instalar e configurar o Zaaheen',
    title: 'Instale e configure o Zaaheen no Windows ou no Mac',
    description:
      'Instale o Zaaheen no Windows ou no Mac, crie sua conta, escolha onde ficam suas memórias e conecte seu primeiro app de IA. A configuração leva uns três minutos.',
  },
  '/docs/subscribe/': {
    crumb: 'Como assinar',
    heading: 'Como assinar o Zaaheen',
    title: 'Como assinar o Zaaheen: US$ 5 por mês ou US$ 48 por ano',
    description:
      'Crie sua conta, baixe o Zaaheen e comece seu teste grátis de 30 dias, depois assine por US$ 5 por mês ou US$ 48 por ano no app ou na sua página Account.',
  },
  '/docs/connect-claude/': {
    crumb: 'Conectar o Claude Desktop',
    heading: 'Como conectar o Claude Desktop ao Zaaheen',
    title: 'Conecte o Claude Desktop ao Zaaheen: memória de IA compartilhada',
    description:
      'Dê ao app do Claude no seu computador uma memória privada que ele compartilha com seus outros apps de IA: um clique no Zaaheen e depois Install no Claude.',
  },
  '/docs/connect-claude-code/': {
    crumb: 'Conectar o Claude Code',
    heading: 'Como conectar o Claude Code ao Zaaheen',
    title: 'Conecte o Claude Code ao Zaaheen: memória entre sessões',
    description:
      'Dê ao Claude Code uma memória que seus outros apps de IA compartilham: rode um comando no terminal, e o Zaaheen estará em todos os projetos do Claude Code.',
  },
  '/docs/connect-cursor/': {
    crumb: 'Conectar o Cursor',
    heading: 'Como conectar o Cursor ao Zaaheen',
    title: 'Conecte o Cursor ao Zaaheen: memória entre sessões',
    description:
      'Dê ao Cursor uma memória que seus outros apps de IA compartilham: um clique no Zaaheen e depois Install no Cursor. Como conferir e como configurar à mão.',
  },
  '/docs/connect-chatgpt/': {
    crumb: 'Conectar o ChatGPT',
    heading: 'Como conectar o ChatGPT ao Zaaheen',
    title: 'Conecte o ChatGPT ao Zaaheen: memória de IA compartilhada',
    description:
      'Conecte o app do ChatGPT para computador ao Zaaheen em Settings, Plugins, MCP, use-o em um chat de Work ou de Codex e faça o ChatGPT consultar o Zaaheen.',
  },
  '/docs/connect-codex/': {
    crumb: 'Conectar o Codex',
    heading: 'Como conectar o Codex ao Zaaheen',
    title: 'Conecte o Codex ao Zaaheen: memória de IA compartilhada',
    description:
      'Dê ao Codex da OpenAI uma memória compartilhada com seus outros apps de IA: adicione o Zaaheen ao config.toml, reinicie e confira em /mcp.',
  },
  '/docs/connect-antigravity/': {
    crumb: 'Conectar o Antigravity',
    heading: 'Como conectar o Antigravity ao Zaaheen',
    title: 'Conecte o Antigravity ao Zaaheen: memória de IA compartilhada',
    description:
      'Dê ao Antigravity do Google uma memória compartilhada com seus outros apps de IA: cole a configuração do Zaaheen no mcp_config.json e reinicie.',
  },
  '/docs/connect-other-apps/': {
    crumb: 'Conectar outros apps de IA',
    heading: 'Como conectar outros apps de IA ao Zaaheen',
    title: 'Conecte outros apps de IA ao Zaaheen com MCP',
    description:
      'O Zaaheen funciona com qualquer app de IA compatível com MCP. Quais apps têm guia próprio, a configuração para qualquer outro app e como conferir a conexão.',
  },
  '/docs/troubleshooting/': {
    crumb: 'Solução de problemas',
    heading: 'Como resolver problemas comuns do Zaaheen',
    title: 'Solução de problemas do Zaaheen: resolva os mais comuns',
    description:
      'O que fazer quando um app de IA não usa sua memória do Zaaheen, não aparece como conectado ou o ChatGPT não a vê, e como nos enviar um registro.',
  },
  '/docs/contact/': {
    crumb: 'Fale conosco',
    heading: 'Fale com o Zaaheen',
    title: 'Fale conosco · Zaaheen',
    description:
      'Onde encontrar respostas sobre o app de memória Zaaheen, como nos enviar um e-mail e o que incluir para podermos ajudar logo na primeira vez.',
  },
} satisfies Record<string, PageMeta>;
