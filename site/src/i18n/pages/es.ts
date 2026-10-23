// Spanish page titles and descriptions (src/i18n/pages.ts), keyed by English path.
import type { PageMeta } from '../pages';

export default {
  '/': {
    title: 'Zaaheen: una memoria privada para Claude, ChatGPT y Cursor',
    description:
      'Zaaheen da a Claude, Cursor, ChatGPT y otras apps de IA una memoria compartida sobre ti, cifrada en tu propia computadora. Para Windows y Mac, 30 días gratis.',
  },
  // Prices and trial days are typed here (PRICES and TRIAL in data/site.ts):
  // importing site.ts from this file would be a circular import.
  '/products/': {
    title: 'Productos: una memoria privada para tus apps de IA · Zaaheen',
    description:
      'Los productos que crea Zaaheen, empezando por una memoria privada y cifrada que tus asistentes de IA comparten en tu propia computadora.',
  },
  '/pricing/': {
    title: 'Precios: 5 USD al mes tras 30 días gratis · Zaaheen',
    description:
      'Precios de Zaaheen: 30 días gratis, sin tarjeta, luego 5 USD al mes o 48 USD al año. Comparado con otras herramientas y con la memoria de ChatGPT y Claude.',
  },
  '/docs/': {
    crumb: 'Documentos',
    title: 'Ayuda y guías de la app de memoria · Zaaheen',
    description:
      'Guías de los productos de Zaaheen: cómo conectar Claude, Cursor y otras apps de IA, mantener tus datos a salvo y resolver los problemas más comunes.',
  },
  '/docs/getting-started/': {
    crumb: 'Primeros pasos',
    heading: 'Cómo instalar y configurar Zaaheen',
    title: 'Instala y configura Zaaheen en Windows o Mac',
    description:
      'Instala Zaaheen en Windows o Mac, crea tu cuenta, elige dónde se guardan tus recuerdos y conecta tu primera app de IA. La configuración tarda unos tres minutos.',
  },
  '/docs/subscribe/': {
    crumb: 'Cómo suscribirte',
    heading: 'Cómo suscribirte a Zaaheen',
    title: 'Cómo suscribirte a Zaaheen: 5 USD al mes o 48 USD al año',
    description:
      'Crea tu cuenta, descarga Zaaheen y empieza tu prueba gratis de 30 días; luego suscríbete por 5 USD al mes o 48 USD al año en la app o en tu página Account.',
  },
  '/docs/connect-claude/': {
    crumb: 'Conectar Claude Desktop',
    heading: 'Cómo conectar Claude Desktop a Zaaheen',
    title: 'Conecta Claude Desktop a Zaaheen: memoria de IA compartida',
    description:
      'Dale a la app de Claude de tu computadora una memoria privada que comparte con tus otras apps de IA: un clic en Zaaheen y luego Install en Claude.',
  },
  '/docs/connect-claude-code/': {
    crumb: 'Conectar Claude Code',
    heading: 'Cómo conectar Claude Code a Zaaheen',
    title: 'Conecta Claude Code a Zaaheen: memoria entre sesiones',
    description:
      'Dale a Claude Code una memoria que comparten tus otras apps de IA: ejecuta un comando en una terminal, y Zaaheen estará en todos tus proyectos de Claude Code.',
  },
  '/docs/connect-cursor/': {
    crumb: 'Conectar Cursor',
    heading: 'Cómo conectar Cursor a Zaaheen',
    title: 'Conecta Cursor a Zaaheen: memoria entre sesiones',
    description:
      'Dale a Cursor una memoria que comparten tus otras apps de IA: un clic en Zaaheen y luego Install en Cursor. Cómo comprobarlo, y la configuración a mano.',
  },
  '/docs/connect-chatgpt/': {
    crumb: 'Conectar ChatGPT',
    heading: 'Cómo conectar ChatGPT a Zaaheen',
    title: 'Conecta ChatGPT a Zaaheen: memoria de IA compartida',
    description:
      'Conecta la app de ChatGPT de tu computadora a Zaaheen en Settings, Plugins, MCP, úsalo en un chat de Work o de Codex, y haz que ChatGPT lo consulte.',
  },
  '/docs/connect-codex/': {
    crumb: 'Conectar Codex',
    heading: 'Cómo conectar Codex a Zaaheen',
    title: 'Conecta Codex a Zaaheen: memoria de IA compartida',
    description:
      'Dale a Codex de OpenAI una memoria que comparten tus otras apps de IA: añade la configuración de Zaaheen a config.toml, reinicia Codex y comprueba /mcp.',
  },
  '/docs/connect-antigravity/': {
    crumb: 'Conectar Antigravity',
    heading: 'Cómo conectar Antigravity a Zaaheen',
    title: 'Conecta Antigravity a Zaaheen: memoria de IA compartida',
    description:
      'Dale a Antigravity de Google una memoria que comparten tus otras apps de IA: pega la configuración de Zaaheen en mcp_config.json, reinicia y comprueba.',
  },
  '/docs/connect-other-apps/': {
    crumb: 'Conectar otras apps de IA',
    heading: 'Cómo conectar otras apps de IA a Zaaheen',
    title: 'Conecta otras apps de IA a Zaaheen con MCP',
    description:
      'Zaaheen funciona con cualquier app de IA compatible con MCP. Qué apps tienen su propia guía, la configuración para cualquier otra y cómo comprobar la conexión.',
  },
  '/docs/troubleshooting/': {
    crumb: 'Solución de problemas',
    heading: 'Cómo resolver problemas comunes con Zaaheen',
    title: 'Solución de problemas de Zaaheen: arregla lo más común',
    description:
      'Qué hacer cuando una app de IA no usa tu memoria de Zaaheen, no aparece como conectada o ChatGPT no la ve, y cómo enviarnos un registro de actividad.',
  },
  '/docs/contact/': {
    crumb: 'Contacto',
    heading: 'Contactar con Zaaheen',
    title: 'Contacto · Zaaheen',
    description:
      'Dónde encontrar respuestas sobre la app de memoria Zaaheen, cómo escribirnos por correo y qué incluir para que podamos ayudarte a la primera.',
  },
} satisfies Record<string, PageMeta>;
