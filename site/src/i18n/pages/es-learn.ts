// es Learn article titles, descriptions, headings, crumbs and summaries (src/i18n/pages.ts), keyed by English path.
import type { PageMeta } from '../pages';

export default {
  '/learn/': {
    crumb: 'Aprende',
    title: 'Aprende: memoria de IA en Claude, ChatGPT y Cursor · Zaaheen',
    description:
      'Respuestas claras sobre la memoria de la IA: cómo te recuerdan Claude, ChatGPT, Claude Code y Cursor, y cómo tener una sola memoria para todas estas apps.',
  },
  '/learn/share-memory-between-chatgpt-and-claude/': {
    crumb: 'Compartir memoria entre ChatGPT y Claude',
    heading: 'Cómo compartir la memoria entre ChatGPT y Claude',
    title: 'Cómo compartir la memoria entre ChatGPT y Claude',
    description:
      'ChatGPT y Claude guardan cada uno su propia memoria y no pueden leer la del otro. Tres formas de llevar lo que saben de ti de una app a la otra.',
    summary:
      'No pueden leer la memoria del otro. Cópiala, usa la importación de Claude o da a las dos apps una sola memoria compartida.',
  },
  '/learn/does-claude-code-remember-between-sessions/': {
    crumb: '¿Claude Code recuerda entre sesiones?',
    heading: '¿Claude Code recuerda entre sesiones?',
    title: '¿Claude Code recuerda entre sesiones? · Zaaheen',
    description:
      'Sí, dentro de un proyecto y en una computadora. Qué guardan CLAUDE.md y la memoria automática de Claude Code, qué se carga en cada sesión y qué no se llevan.',
    summary:
      'Sí, dentro de un proyecto y en una computadora. Qué guardan CLAUDE.md y la memoria automática, qué se carga en cada sesión y qué se queda atrás.',
  },
  '/learn/one-memory-across-claude-code-cursor-and-codex/': {
    crumb: 'Una memoria para Claude Code, Cursor y Codex',
    heading: 'Cómo tener una sola memoria en Claude Code, Cursor y Codex',
    title: 'Una memoria para Claude Code, Cursor y Codex · Zaaheen',
    description:
      'Claude Code, Cursor y Codex guardan cada uno su propia memoria. Comparte las reglas del proyecto con un AGENTS.md, y lo que es cierto sobre ti con una memoria.',
    summary:
      'Cada uno guarda su propia memoria. Comparte las reglas del proyecto con un AGENTS.md, y lo que saben de ti con una sola memoria compartida.',
  },
  '/learn/ai-memory-tool-or-notes-file/': {
    crumb: '¿Herramienta de memoria o archivo de notas?',
    heading: '¿Necesitas una herramienta de memoria para la IA, o basta un archivo de notas?',
    title: '¿Herramienta de memoria o archivo de notas? · Zaaheen',
    description:
      'Para un proyecto en una herramienta de programación, CLAUDE.md o AGENTS.md suele bastar. Cuándo deja de bastar un archivo de notas y cómo usar los dos.',
    summary:
      'Suele bastar para un proyecto en una herramienta. Dónde se queda corto un archivo y por qué probablemente querrás los dos.',
  },
  '/learn/switch-from-chatgpt-to-claude/': {
    crumb: 'Pasar de ChatGPT a Claude',
    heading: 'Cómo pasar de ChatGPT a Claude sin empezar de cero',
    title: 'Pasa de ChatGPT a Claude sin empezar de cero · Zaaheen',
    description:
      'Lleva a Claude tu memoria de ChatGPT, tus instrucciones personalizadas y tus chats antiguos: los pasos, cuánto tarda cada uno y cómo no volver a moverlos nunca.',
    summary:
      'Lleva a Claude tu memoria, tus instrucciones y tus chats antiguos de ChatGPT, cuánto tarda cada paso y cómo no volver a moverlos nunca.',
  },
  '/learn/ai-memory-apps-compared/': {
    crumb: 'Apps de memoria para IA comparadas',
    heading: 'Apps de memoria para IA comparadas: cuáles funcionan en ChatGPT, Claude y Cursor',
    title: 'Apps de memoria para IA: Supermemory, Mem0 y más · Zaaheen',
    description:
      'Apps que dan a ChatGPT, Claude y Cursor una memoria compartida, comparadas por dónde guardan los recuerdos, configuración y precio, según cada empresa.',
    summary:
      'Cinco apps que dan a tus apps de IA una memoria compartida, lado a lado: dónde guardan los recuerdos, a qué apps llegan, configuración y precio.',
  },
  '/learn/chatgpt-memory-vs-claude-memory/': {
    crumb: 'Memoria de ChatGPT frente a la de Claude',
    heading: 'La memoria de ChatGPT frente a la de Claude: cómo te recuerda cada una',
    title: 'Memoria de ChatGPT frente a la de Claude, lado a lado · Zaaheen',
    description:
      'Cómo te recuerdan ChatGPT y Claude, qué puedes ver y cambiar, cómo tratan los datos sensibles y qué hacer si usas los dos, según la ayuda de cada empresa.',
    summary:
      'En qué se basa cada una, qué puedes ver y cambiar, los datos sensibles y qué hacer si usas los dos.',
  },
  '/learn/mem0-alternative/': {
    crumb: 'Alternativa a Mem0',
    heading: 'Una alternativa a Mem0 para quien simplemente usa apps de IA',
    title: 'Alternativa a Mem0: una memoria privada, sin programar · Zaaheen',
    description:
      'Mem0 es memoria para desarrolladores, guardada en tu cuenta de Mem0. En qué se diferencia Zaaheen, cuándo Mem0 es mejor opción y dónde se queda corto Zaaheen.',
    summary:
      'Mem0 está hecho para desarrolladores y guarda los recuerdos en tu cuenta de Mem0. Cuándo te conviene más una app en tu propia computadora.',
  },
  '/learn/supermemory-alternative/': {
    crumb: 'Alternativa a Supermemory',
    heading: 'Una alternativa a Supermemory que se instala como una app normal',
    title: 'Alternativa a Supermemory: cifrada, sin terminal · Zaaheen',
    description:
      'Supermemory es un motor de memoria sobre todo para desarrolladores. En qué se diferencia Zaaheen, cuándo Supermemory es mejor y dónde se queda corto Zaaheen.',
    summary:
      'Supermemory es un motor de memoria sobre todo para desarrolladores. Cuándo te conviene más una app cifrada y sin terminal.',
  },
  '/learn/memoryplugin-alternative/': {
    crumb: 'Alternativa a MemoryPlugin',
    heading: 'Una alternativa a MemoryPlugin que guarda tus recuerdos en tu computadora',
    title: 'Alternativa a MemoryPlugin: recuerdos en tu computadora · Zaaheen',
    description:
      'MemoryPlugin guarda una memoria para más de 21 herramientas de IA en su servicio. Zaaheen comparado en almacenamiento, apps y precio, y cuál te conviene.',
    summary:
      'MemoryPlugin llega a más de 21 herramientas de IA desde su propio servicio. Cuándo importa más guardar los recuerdos en tu computadora.',
  },
  '/learn/best-memory-mcp-server-for-claude-code/': {
    crumb: 'Servidores de memoria para Claude Code',
    heading: '¿Qué servidor MCP de memoria usar con Claude Code?',
    title: '¿El mejor servidor MCP de memoria para Claude Code? · Zaaheen',
    description:
      'El servidor de referencia de Anthropic, mcp-memory-service, Basic Memory, Mem0 y Zaaheen para Claude Code: configuración, dónde viven los recuerdos y costo.',
    summary:
      'Los servidores gratis, Basic Memory, Mem0 y Zaaheen para Claude Code: configuración, dónde viven los recuerdos y costo.',
  },
  '/learn/obsidian-vs-ai-memory-app/': {
    crumb: 'Obsidian frente a una app de memoria',
    heading: '¿Obsidian o una app de memoria para tu IA?',
    title: 'Obsidian como memoria de IA frente a una app de memoria · Zaaheen',
    description:
      'Obsidian son notas que escribes tú; en una app de memoria escriben tus apps de IA mientras hablas. Para qué sirve cada una, cuánto cuestan y cómo usar las dos.',
    summary:
      'Notas que escribes tú frente a una memoria que escriben tus apps de IA mientras hablas. Para qué sirve cada una, el costo y cómo usar las dos.',
  },
} satisfies Record<string, PageMeta>;
