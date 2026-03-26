import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, ReactNode } from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Copy, ExternalLink, Folder, Scissors, Search, Trash2, X } from 'lucide-react'
import { getFileFloat, type SearchResult, type SnapState } from './fileFloat'

function useDebounce<T>(value: T, delay: number): T {
  const [debouncedValue, setDebouncedValue] = useState(value)
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedValue(value), delay)
    return () => clearTimeout(timer)
  }, [value, delay])
  return debouncedValue
}

export default function App() {
  const fileFloat = getFileFloat()
  const expandedWidth = 380
  const minExpandedHeight = 120
  const [isExpanded, setIsExpanded] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<SearchResult[]>([])
  const [isLoading, setIsLoading] = useState(false)
  const [selectedIndex, setSelectedIndex] = useState(-1)
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)
  const [, setSnapState] = useState<SnapState>('none')
  const [isIconHovered, setIsIconHovered] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const dragState = useRef<{ startX: number; startY: number; moved: boolean } | null>(null)
  const isFinishingDrag = useRef(false)
  const collapseTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const iconCollapseTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const menuActionTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const isInputFocused = useRef(false)
  const isPanelHovered = useRef(false)
  const isContextMenuOpen = useRef(false)
  const debouncedQuery = useDebounce(query, 220)

  const clearTimer = (ref: typeof collapseTimer | typeof iconCollapseTimer) => {
    if (ref.current) {
      clearTimeout(ref.current)
      ref.current = null
    }
  }

  const clearMenuActionTimer = useCallback(() => {
    if (menuActionTimer.current) {
      clearTimeout(menuActionTimer.current)
      menuActionTimer.current = null
    }
  }, [])

  const keepPanelAlive = useCallback((duration = 1000) => {
    clearTimer(collapseTimer)
    clearTimer(iconCollapseTimer)
    clearMenuActionTimer()
    isContextMenuOpen.current = true
    menuActionTimer.current = window.setTimeout(() => {
      isContextMenuOpen.current = false
    }, duration)
  }, [clearMenuActionTimer])

  const handleClose = useCallback(async () => {
    clearTimer(collapseTimer)
    clearTimer(iconCollapseTimer)
    clearMenuActionTimer()
    isInputFocused.current = false
    isPanelHovered.current = false
    isContextMenuOpen.current = false
    setContextMenu(null)
    setQuery('')
    setResults([])
    setSelectedIndex(-1)

    if (isExpanded) {
      setIsExpanded(false)
      await fileFloat.restoreCollapsedWindow?.()
    }
  }, [clearMenuActionTimer, fileFloat, isExpanded])

  const handleIconClick = useCallback(async () => {
    clearTimer(iconCollapseTimer)
    clearTimer(collapseTimer)
    clearMenuActionTimer()

    await fileFloat.ensureSafePosition?.(expandedWidth, minExpandedHeight)
    await fileFloat.setWindowSize?.(expandedWidth, minExpandedHeight)
    setIsExpanded(true)
    window.setTimeout(() => inputRef.current?.focus(), 60)
  }, [clearMenuActionTimer, expandedWidth, fileFloat, minExpandedHeight])

  const finishDrag = useCallback(async (allowClick: boolean) => {
    if (!dragState.current || isFinishingDrag.current) return

    isFinishingDrag.current = true
    const localMoved = dragState.current.moved
    dragState.current = null

    try {
      const result = await fileFloat.dragEnd?.()
      const wasMoved = Boolean(localMoved || result?.moved)
      if (allowClick && !wasMoved) {
        await handleIconClick()
      }
    } finally {
      isFinishingDrag.current = false
    }
  }, [fileFloat, handleIconClick])

  useEffect(() => {
    const onGlobalMouseUp = () => {
      if (dragState.current) {
        void finishDrag(false)
      }
    }
    window.addEventListener('mouseup', onGlobalMouseUp)
    return () => window.removeEventListener('mouseup', onGlobalMouseUp)
  }, [finishDrag])

  const handleIconMouseDown = useCallback((e: ReactMouseEvent) => {
    e.preventDefault()
    dragState.current = { startX: e.clientX, startY: e.clientY, moved: false }
    fileFloat.dragStart?.()
  }, [fileFloat])

  const handleIconMouseMove = useCallback((e: ReactMouseEvent) => {
    if (!dragState.current) return
    const dx = e.clientX - dragState.current.startX
    const dy = e.clientY - dragState.current.startY
    if (Math.sqrt(dx * dx + dy * dy) > 8) {
      dragState.current.moved = true
    }
  }, [])

  const handleIconMouseUp = useCallback(() => {
    void finishDrag(true)
  }, [finishDrag])

  useEffect(() => {
    const cleanup = fileFloat.onSnapState?.((data) => {
      setSnapState(data.snapState)
    })
    return () => cleanup?.()
  }, [fileFloat])

  useEffect(() => {
    if (!isExpanded) return

    clearTimer(iconCollapseTimer)
    clearTimer(collapseTimer)

    const itemH = 56
    const headerH = 54
    const padding = 16
    const height = Math.min(headerH + padding + results.length * itemH, headerH + padding + 8 * itemH)
    fileFloat.setWindowSize?.(expandedWidth, Math.max(minExpandedHeight, height))
  }, [expandedWidth, fileFloat, isExpanded, minExpandedHeight, results.length])

  useEffect(() => {
    let cancelled = false
    const runSearch = async () => {
      const term = debouncedQuery.trim()
      if (!term) {
        setResults([])
        setIsLoading(false)
        setSelectedIndex(-1)
        setContextMenu(null)
        return
      }

      setIsLoading(true)
      try {
        const next = await fileFloat.searchFiles(term)
        if (!cancelled) {
          const normalized = Array.isArray(next) ? next : []
          setResults(normalized)
          setSelectedIndex(normalized.length > 0 ? 0 : -1)
          setContextMenu(null)
        }
      } catch {
        if (!cancelled) {
          setResults([])
          setSelectedIndex(-1)
          setContextMenu(null)
        }
      } finally {
        if (!cancelled) setIsLoading(false)
      }
    }

    void runSearch()
    return () => {
      cancelled = true
    }
  }, [debouncedQuery, fileFloat])

  const handlePanelMouseLeave = useCallback(() => {
    if (isContextMenuOpen.current) return
    isPanelHovered.current = false
    clearTimer(collapseTimer)
    collapseTimer.current = window.setTimeout(() => {
      if (document.activeElement === inputRef.current) {
        inputRef.current?.blur()
      }
      void handleClose()
    }, 60000)
  }, [handleClose])

  const handlePanelMouseEnter = useCallback(() => {
    isPanelHovered.current = true
    clearTimer(collapseTimer)
  }, [])

  const handleInputFocus = useCallback(() => {
    isInputFocused.current = true
    clearTimer(collapseTimer)
    clearTimer(iconCollapseTimer)
  }, [])

  const handleInputBlur = useCallback(() => {
    isInputFocused.current = false
    if (!isPanelHovered.current && !isContextMenuOpen.current) {
      clearTimer(collapseTimer)
      collapseTimer.current = window.setTimeout(() => {
        void handleClose()
      }, 60000)
    }
  }, [handleClose])

  const handleIconMouseLeave = useCallback(() => {
    if (isContextMenuOpen.current) return
    if (dragState.current) return
    setIsIconHovered(false)
    clearTimer(iconCollapseTimer)
    iconCollapseTimer.current = window.setTimeout(() => {
      void handleClose()
    }, 500)
  }, [handleClose])

  const handleIconMouseEnter = useCallback(() => {
    setIsIconHovered(true)
    clearTimer(iconCollapseTimer)
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (contextMenu) {
          setContextMenu(null)
          isContextMenuOpen.current = false
          clearMenuActionTimer()
          return
        }
        void handleClose()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [clearMenuActionTimer, contextMenu, handleClose])

  useEffect(() => {
    if (selectedIndex < 0) return
    const row = document.querySelector<HTMLElement>(`[data-result-index="${selectedIndex}"]`)
    row?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  useEffect(() => {
    const onBlur = () => {
      if (!isExpanded) return
      if (isContextMenuOpen.current) return
      clearTimer(collapseTimer)
      collapseTimer.current = window.setTimeout(() => {
        void handleClose()
      }, 60000)
    }
    window.addEventListener('blur', onBlur)
    return () => window.removeEventListener('blur', onBlur)
  }, [handleClose, isExpanded])

  useEffect(() => {
    return () => {
      clearTimer(collapseTimer)
      clearTimer(iconCollapseTimer)
      clearMenuActionTimer()
    }
  }, [clearMenuActionTimer])

  const getFileIcon = (kind?: string): string => {
    if (!kind) return '📄'
    const k = kind.toLowerCase()
    if (k.includes('folder') || k.includes('directory')) return '📁'
    if (k.includes('picture') || k.includes('image') || k.includes('photo')) return '🖼️'
    if (k.includes('video') || k.includes('movie')) return '🎬'
    if (k.includes('music') || k.includes('audio') || k.includes('sound')) return '🎵'
    if (k.includes('document') || k.includes('text')) return '📝'
    if (k.includes('program') || k.includes('application')) return '⚙️'
    return '📄'
  }

  const isFolderResult = useCallback((result: SearchResult) => {
    const kind = (result.kind ?? '').toLowerCase()
    const name = result.name.trim()
    const path = result.path.trim()
    const hasFileLikeSuffix = /\.[^\\/.]+$/.test(name) || /\.[^\\/.]+$/.test(path)
    if (hasFileLikeSuffix) return false
    if (kind.includes('folder') || kind.includes('directory')) return true
    return path.endsWith('\\') || path.endsWith('/')
  }, [])

  const openResult = useCallback((result: SearchResult) => {
    if (isFolderResult(result)) {
      void fileFloat.openFile?.(result.path)
      return
    }
    void fileFloat.openFile?.(result.path)
  }, [fileFloat, isFolderResult])

  const openContainingDirectory = useCallback((result: SearchResult) => {
    void fileFloat.showInFolder?.(result.path)
  }, [fileFloat])

  const copyItemPath = useCallback((result: SearchResult) => {
    void fileFloat.copyText?.(result.path)
  }, [fileFloat])

  const copyItem = useCallback((result: SearchResult, cut: boolean) => {
    void fileFloat.copyItem?.(result.path, cut)
  }, [fileFloat])

  const deleteItem = useCallback(async (result: SearchResult) => {
    await fileFloat.deleteItem?.(result.path)
    setResults((current) => {
      const next = current.filter((item) => item.path !== result.path)
      setSelectedIndex((selected) => {
        if (next.length === 0) return -1
        if (selected < 0) return 0
        return Math.min(selected, next.length - 1)
      })
      return next
    })
  }, [fileFloat])

  const runMenuAction = useCallback((action: () => void | Promise<void>) => {
    keepPanelAlive(1200)
    window.setTimeout(() => {
      void Promise.resolve(action()).finally(() => {
        window.setTimeout(() => {
          setContextMenu(null)
        }, 80)
      })
    }, 0)
  }, [keepPanelAlive])

  const openContextMenu = useCallback((result: SearchResult, index: number, x: number, y: number) => {
    keepPanelAlive(1600)
    const menuWidth = 276
    const menuHeight = result.kind?.toLowerCase().includes('folder') ? 238 : 274
    const menuX = Math.min(x, Math.max(8, window.innerWidth - menuWidth - 8))
    const menuY = Math.min(y, Math.max(8, window.innerHeight - menuHeight - 8))
    setSelectedIndex(index)
    setContextMenu({ result, index, x: menuX, y: menuY })
  }, [keepPanelAlive])

  const closeContextMenu = useCallback(() => {
    clearMenuActionTimer()
    isContextMenuOpen.current = false
    setContextMenu(null)
  }, [clearMenuActionTimer])

  const handleInputKeyDown = useCallback((e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (results.length === 0) return

    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSelectedIndex((current) => {
        if (current < 0) return 0
        return (current + 1) % results.length
      })
      return
    }

    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSelectedIndex((current) => {
        if (current < 0) return results.length - 1
        return (current - 1 + results.length) % results.length
      })
      return
    }

    if (e.key === 'Enter') {
      e.preventDefault()
      const index = selectedIndex >= 0 && selectedIndex < results.length ? selectedIndex : 0
      const target = results[index]
      if (target) {
        openResult(target)
      }
    }
  }, [openResult, results, selectedIndex])

  const handleResultActivate = useCallback((result: SearchResult, index: number) => {
    setSelectedIndex(index)
    openResult(result)
  }, [openResult])

  const handleCloseRef = useRef(handleClose)
  handleCloseRef.current = handleClose
  const handleIconClickRef = useRef(handleIconClick)
  handleIconClickRef.current = handleIconClick
  const isExpandedRef = useRef(isExpanded)
  isExpandedRef.current = isExpanded

  useEffect(() => {
    const onShortcutToggle = () => {
      if (isExpandedRef.current) {
        void handleCloseRef.current()
      } else {
        void handleIconClickRef.current()
      }
    }
    window.addEventListener('filefloat-shortcut-toggle', onShortcutToggle)
    return () => window.removeEventListener('filefloat-shortcut-toggle', onShortcutToggle)
  }, [])

  return (
    <div
      className="shell"
      data-state={isExpanded ? 'expanded' : 'collapsed'}
      onContextMenu={(e) => e.preventDefault()}
    >
      {!isExpanded ? (
        <button
          className="orb"
          type="button"
          aria-label="Open search"
          aria-pressed={false}
          data-hovered={isIconHovered ? 'true' : 'false'}
          onMouseEnter={handleIconMouseEnter}
          onMouseLeave={handleIconMouseLeave}
          onMouseDown={handleIconMouseDown}
          onMouseMove={handleIconMouseMove}
          onMouseUp={handleIconMouseUp}
        >
          <Search className="orb__glyph" size={24} strokeWidth={2.2} />
        </button>
      ) : (
        <section
          className="panel"
          onMouseEnter={handlePanelMouseEnter}
          onMouseLeave={handlePanelMouseLeave}
          aria-label="FileFloat search panel"
        >
          <div className="search-header">
            <button
              className="drag-handle"
              type="button"
              onClick={() => void handleClose()}
              aria-label="收起搜索"
              title="收起"
            >
              <Search size={16} />
              <div className="search-active-dot" />
            </button>

            <input
              ref={inputRef}
              id="search-input"
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={handleInputKeyDown}
              onFocus={handleInputFocus}
              onBlur={handleInputBlur}
              placeholder="搜索文件名..."
              className="search-header__input"
            />

            {isLoading ? <span className="spinner" aria-label="loading" /> : null}

            <button className="close-btn" type="button" onClick={() => void handleClose()} aria-label="关闭搜索">
              <X size={15} />
            </button>
          </div>

          {(results.length > 0 || (query.length >= 1 && !isLoading)) && <div className="divider" />}

          {results.length > 0 ? (
            <div className="result-list">
              {results.map((result, index) => (
                <ResultItem
                  key={`${result.path}-${result.name}`}
                  index={index}
                  result={result}
                  icon={getFileIcon(result.kind)}
                  isSelected={index === selectedIndex}
                  isFolder={isFolderResult(result)}
                  onSelect={() => setSelectedIndex(index)}
                  onActivate={() => handleResultActivate(result, index)}
                  onOpenFolder={() => openContainingDirectory(result)}
                  onOpen={() => openResult(result)}
                  onContextMenu={(x, y) => openContextMenu(result, index, x, y)}
                />
              ))}
            </div>
          ) : null}

          {!isLoading && query.length >= 1 && results.length === 0 ? (
            <div className="empty-state">未找到匹配的文件</div>
          ) : null}

          {query.length === 0 ? <div className="hint-state">输入文件名开始搜索</div> : null}

          {contextMenu ? (
            <ContextMenu
              x={contextMenu.x}
              y={contextMenu.y}
              result={contextMenu.result}
              isFolder={isFolderResult(contextMenu.result)}
              onClose={closeContextMenu}
              onKeepAlive={keepPanelAlive}
              onOpen={() => runMenuAction(() => openResult(contextMenu.result))}
              onOpenFolder={() => runMenuAction(() => openContainingDirectory(contextMenu.result))}
              onCopy={() => runMenuAction(() => copyItem(contextMenu.result, false))}
              onCut={() => runMenuAction(() => copyItem(contextMenu.result, true))}
              onDelete={() => runMenuAction(async () => deleteItem(contextMenu.result))}
              onCopyFullPath={() => runMenuAction(() => copyItemPath(contextMenu.result))}
            />
          ) : null}
        </section>
      )}
    </div>
  )
}

type ContextMenuState = {
  result: SearchResult
  index: number
  x: number
  y: number
}

interface ResultItemProps {
  index: number
  result: SearchResult
  icon: string
  isSelected: boolean
  isFolder: boolean
  onSelect: () => void
  onActivate: () => void
  onOpenFolder: () => void
  onOpen: () => void
  onContextMenu: (x: number, y: number) => void
}

function ResultItem({
  index,
  result,
  icon,
  isSelected,
  isFolder,
  onSelect,
  onActivate,
  onOpenFolder,
  onOpen,
  onContextMenu,
}: ResultItemProps) {
  const [hovered, setHovered] = useState(false)

  return (
    <div
      className="result-row"
      data-hovered={hovered ? 'true' : 'false'}
      data-selected={isSelected ? 'true' : 'false'}
      data-result-index={index}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={onSelect}
      onDoubleClick={onActivate}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
        onContextMenu(e.clientX, e.clientY)
      }}
      role="button"
      tabIndex={0}
    >
      <span className="result-row__icon">{icon}</span>
      <div className="result-row__body">
        <div className="result-row__title">{result.name}</div>
        <div className="result-row__meta" title={result.path}>
          {result.path}
        </div>
      </div>
      {!isFolder ? (
        <div className="result-row__actions">
          <ActionBtn title="打开所在目录" onClick={(e) => { e.stopPropagation(); onOpenFolder() }}>
            <Folder size={20} />
          </ActionBtn>
          <ActionBtn title="打开文件" onClick={(e) => { e.stopPropagation(); onOpen() }}>
            <ExternalLink size={20} />
          </ActionBtn>
        </div>
      ) : null}
    </div>
  )
}

interface ActionBtnProps {
  title: string
  onClick: (e: ReactMouseEvent) => void
  children: ReactNode
}

function ActionBtn({ title, onClick, children }: ActionBtnProps) {
  const [hovered, setHovered] = useState(false)

  return (
    <button
      type="button"
      className="action-btn"
      title={title}
      onClick={onClick}
      onDoubleClick={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      data-hovered={hovered ? 'true' : 'false'}
    >
      {children}
    </button>
  )
}

interface ContextMenuProps {
  x: number
  y: number
  result: SearchResult
  isFolder: boolean
  onClose: () => void
  onKeepAlive: (duration?: number) => void
  onOpen: () => void
  onOpenFolder: () => void
  onCopy: () => void
  onCut: () => void
  onDelete: () => void
  onCopyFullPath: () => void
}

function ContextMenu({
  x,
  y,
  result,
  isFolder,
  onClose,
  onKeepAlive,
  onOpen,
  onOpenFolder,
  onCopy,
  onCut,
  onDelete,
  onCopyFullPath,
}: ContextMenuProps) {
  useEffect(() => {
    const close = () => onClose()
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        close()
      }
    }
    const onMouseDown = () => close()

    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('scroll', close, true)
    window.addEventListener('resize', close)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('scroll', close, true)
      window.removeEventListener('resize', close)
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])

  const stop = (e: ReactMouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }

  const run = (action: () => void) => {
    onKeepAlive(1200)
    action()
    window.setTimeout(onClose, 80)
  }

  return (
    <div
      className="context-menu"
      style={{ left: x, top: y }}
      onMouseEnter={() => onKeepAlive(1200)}
      onMouseMove={() => onKeepAlive(900)}
      onMouseDown={stop}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
      }}
    >
      <div className="context-menu__title" title={result.path}>
        {result.name}
      </div>
      <div className="context-menu__section">
        <MenuAction label={isFolder ? '打开文件夹' : '打开文件'} icon={<ExternalLink size={14} />} onClick={() => run(onOpen)} />
        <MenuAction
          label={isFolder ? '打开所在位置' : '打开所在目录'}
          icon={<Folder size={14} />}
          onClick={() => run(onOpenFolder)}
        />
      </div>
      <div className="context-menu__divider" />
      <div className="context-menu__section">
        <MenuAction label="复制" icon={<Copy size={14} />} onClick={() => run(onCopy)} />
        <MenuAction label="剪切" icon={<Scissors size={14} />} onClick={() => run(onCut)} />
        <MenuAction label="删除" icon={<Trash2 size={14} />} onClick={() => run(onDelete)} />
        <MenuAction label="复制完整路径" icon={<Copy size={14} />} onClick={() => run(onCopyFullPath)} />
      </div>
    </div>
  )
}

interface MenuActionProps {
  label: string
  icon: ReactNode
  onClick: () => void
}

function MenuAction({ label, icon, onClick }: MenuActionProps) {
  return (
    <button
      type="button"
      className="context-menu__item"
      onMouseDown={(e) => {
        e.preventDefault()
        e.stopPropagation()
      }}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        onClick()
      }}
    >
      <span className="context-menu__icon" aria-hidden="true">{icon}</span>
      <span className="context-menu__label">{label}</span>
    </button>
  )
}
