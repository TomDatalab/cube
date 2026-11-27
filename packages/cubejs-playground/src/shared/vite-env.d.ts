// The parts of Vite's client types the app uses (vite/client would clash
// with the react-scripts declarations).
declare module '*?url' {
  const url: string;
  export default url;
}

interface ImportMetaEnv {
  readonly MODE: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
