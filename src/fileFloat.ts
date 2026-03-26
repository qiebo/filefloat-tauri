import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'

export interface SearchResult {
  name: string
  path: string
  kind?: string
}

export type SnapState = 'none' | 'left' | 'right' | 'top' | 'bottom'

export interface FileFloatAPI {
  searchFiles(query: string): Promise<SearchResult[]>
  openFile(path: string): Promise<void>
  showInFolder(path: string): Promise<void>
  copyItem(path: string, cut: boolean): Promise<void>
  deleteItem(path: string): Promise<void>
  copyText(text: string): Promise<void>
  getAutoStartEnabled(): Promise<boolean>
  setAutoStartEnabled(enabled: boolean): Promise<void>
  setWindowSize(width: number, height: number): Promise<void>
  setWindowPosition(x: number, y: number): Promise<void>
  setAlwaysOnTop(flag: boolean): Promise<void>
  showWindow(): void
  hideWindow(): void
  dragStart(): void
  dragEnd(): Promise<{ moved: boolean }>
  ensureSafePosition(panelW: number, panelH: number): Promise<void>
  restoreCollapsedWindow(): Promise<void>
  getWindowBounds(): Promise<{ x: number; y: number; width: number; height: number }>
  getScreenBounds(): Promise<{ x: number; y: number; width: number; height: number }>
  onSnapState(callback: (data: { snapState: SnapState; isSnapped: boolean }) => void): (() => void) | undefined
  onShortcutToggle(callback: () => void): (() => void) | undefined
}

const demoResults: SearchResult[] = [
  { name: 'project-plan.md', path: 'C:\\Users\\DELL\\Documents\\project-plan.md', kind: 'document' },
  { name: 'design-notes.md', path: 'C:\\Users\\DELL\\Documents\\design-notes.md', kind: 'document' },
  { name: 'invoice-2026.pdf', path: 'C:\\Users\\DELL\\Downloads\\invoice-2026.pdf', kind: 'document' },
  { name: 'screenshot-001.png', path: 'C:\\Users\\DELL\\Pictures\\screenshot-001.png', kind: 'image' },
  { name: 'photo-library', path: 'D:\\Media\\photo-library', kind: 'folder' },
  { name: 'README.txt', path: 'C:\\Users\\DELL\\Desktop\\README.txt', kind: 'text' },
]

const fallbackApi: FileFloatAPI = {
  searchFiles: async (query: string) => {
    const needle = query.trim().toLowerCase()
    if (!needle) return []
    await new Promise((resolve) => setTimeout(resolve, 120))
    return demoResults.filter((item) => {
      const haystack = `${item.name} ${item.path} ${item.kind ?? ''}`.toLowerCase()
      return haystack.includes(needle)
    })
  },
  openFile: async () => {},
  showInFolder: async () => {},
  copyItem: async () => {},
  deleteItem: async () => {},
  copyText: async () => {},
  getAutoStartEnabled: async () => false,
  setAutoStartEnabled: async () => {},
  setWindowSize: async () => {},
  setWindowPosition: async () => {},
  setAlwaysOnTop: async () => {},
  showWindow: () => {},
  hideWindow: () => {},
  dragStart: () => {},
  dragEnd: async () => ({ moved: false }),
  ensureSafePosition: async () => {},
  restoreCollapsedWindow: async () => {},
  getWindowBounds: async () => ({ x: 0, y: 0, width: 520, height: 220 }),
  getScreenBounds: async () => ({ x: 0, y: 0, width: 1920, height: 1080 }),
  onSnapState: () => undefined,
  onShortcutToggle: () => undefined,
}

let cachedApi: FileFloatAPI | null = null

function buildTauriApi(): Partial<FileFloatAPI> {
  const appWindow = getCurrentWindow()

  return {
    searchFiles: (query) => invoke<SearchResult[]>('search_files', { query }),
    openFile: (path) => invoke('open_file', { path }),
    showInFolder: (path) => invoke('show_in_folder', { path }),
    copyItem: (path, cut) => invoke('copy_item', { path, cut }),
    deleteItem: (path) => invoke('delete_item', { path }),
    copyText: (text) => invoke('copy_text', { text }),
    getAutoStartEnabled: () => invoke<boolean>('get_auto_start_enabled'),
    setAutoStartEnabled: (enabled) => invoke('set_auto_start_enabled', { enabled }),
    setWindowSize: (width, height) =>
      invoke('set_window_size', { width, height }),
    setWindowPosition: (x, y) =>
      invoke('set_window_position', { x, y }),
    setAlwaysOnTop: (flag) => appWindow.setAlwaysOnTop(flag),
    showWindow: () => {
      void appWindow.show()
      void appWindow.setFocus()
    },
    hideWindow: () => {
      void appWindow.hide()
    },
    dragStart: () => {
      void invoke('drag_start')
    },
    dragEnd: () => invoke<{ moved: boolean }>('drag_end'),
    ensureSafePosition: (panelW, panelH) =>
      invoke('ensure_safe_position', { panelW, panelH }),
    restoreCollapsedWindow: () => invoke('restore_collapsed_window'),
    getWindowBounds: () => invoke<{ x: number; y: number; width: number; height: number }>('get_window_bounds'),
    getScreenBounds: async () => {
      const monitor = await appWindow.currentMonitor()
      if (!monitor) return { x: 0, y: 0, width: 1920, height: 1080 }
      const { position, size } = monitor.workArea
      return { x: position.x, y: position.y, width: size.width, height: size.height }
    },
    onSnapState: (callback) => {
      const unlisten = listen<{ snapState: SnapState; isSnapped: boolean }>('snap-state', (event) => {
        callback(event.payload)
      })
      return () => {
        void unlisten.then((dispose) => dispose())
      }
    },
    onShortcutToggle: (callback) => {
      const unlisten = listen('shortcut-toggle', () => {
        callback()
      })
      return () => {
        void unlisten.then((dispose) => dispose())
      }
    },
  }
}

export function getFileFloat(): FileFloatAPI {
  if (cachedApi) {
    return cachedApi
  }
  const bridge = window.fileFloat ?? buildTauriApi()
  window.fileFloat = bridge
  cachedApi = { ...fallbackApi, ...bridge } as FileFloatAPI
  return cachedApi
}
