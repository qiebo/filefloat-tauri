import type { FileFloatAPI } from './fileFloat'

declare global {
  interface Window {
    fileFloat?: Partial<FileFloatAPI>
  }
}

export {}
