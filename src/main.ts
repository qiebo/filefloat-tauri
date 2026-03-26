import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import './style.css'

const root = document.querySelector<HTMLDivElement>('#app')

if (!root) {
  throw new Error('App root not found')
}

createRoot(root).render(createElement(App))
