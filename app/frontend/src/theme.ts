const STORAGE_KEY = 'odoru:theme'

type Theme = 'light' | 'dark'

function applyTheme(theme: Theme) {
  if (theme === 'light') document.documentElement.dataset.theme = 'light'
  else delete document.documentElement.dataset.theme
  localStorage.setItem(STORAGE_KEY, theme)
}

export function initTheme() {
  const saved = localStorage.getItem(STORAGE_KEY)
  const theme: Theme = saved === 'light' ? 'light' : 'dark'
  applyTheme(theme)

  const btn = document.createElement('button')
  btn.id = 'theme-toggle'
  btn.className = 'theme-toggle'
  btn.setAttribute('aria-label', 'Toggle light/dark theme')
  btn.textContent = theme === 'light' ? '☀' : '☾'
  btn.addEventListener('click', () => {
    const next: Theme = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light'
    applyTheme(next)
    btn.textContent = next === 'light' ? '☀' : '☾'
  })
  document.body.appendChild(btn)
}
