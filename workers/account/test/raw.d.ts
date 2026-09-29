// Vite's `?raw` import: the file's text, bundled at test build time (deploy-config.test.ts).
declare module "*?raw" {
  const content: string;
  export default content;
}
