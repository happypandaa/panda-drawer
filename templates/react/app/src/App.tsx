import { useCallback, useEffect, useState } from 'react'
import { platform, PlatformError } from '@local-projects/sdk'
import type { ProjectContext } from '@local-projects/sdk'
import { Badge, Button, Card } from './components/ui'

function message(error: unknown) {
  return error instanceof PlatformError ? `${error.message} (${error.code})` : '读取项目时发生错误，请重试。'
}

export default function App() {
  const [context, setContext] = useState<ProjectContext>()
  const [tableCount, setTableCount] = useState<number>()
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [updated, setUpdated] = useState(false)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const current = await platform.context.get()
      const schema = await platform.db.schema()
      setContext(current)
      setTableCount(schema.tables.length)
      setError('')
      document.title = current.name
    } catch (cause) { setError(message(cause)) }
    finally { setLoading(false) }
  }, [])

  useEffect(() => {
    void refresh()
    return platform.db.subscribe(event => {
      if (event.type === 'data.changed') void refresh()
      if (event.type === 'project.updated') setUpdated(true)
      if (event.type === 'error') setError(message(event.error))
    })
  }, [refresh])

  return <main className="page">
    <header className="topbar">
      <div className="wordmark"><span className="project-mark" aria-hidden="true">▧</span><span>{context?.name ?? '我的项目'}</span></div>
      <Badge>{loading ? '连接中' : error ? '需要连接' : '本地已连接'}</Badge>
    </header>

    {updated && <div className="notice" role="status"><span>新的网页已准备好，当前页面会继续保留。</span><Button onClick={() => window.location.reload()}>重新加载</Button></div>}
    {error && <div className="notice error" role="alert"><span>{error}</span><Button onClick={() => void refresh()} disabled={loading}>重新连接</Button></div>}

    <section className="hero">
      <p className="eyebrow">YOUR LOCAL SPACE</p>
      <h1>从一个想法开始。</h1>
      <p className="intro">学习计划、家庭资料，或日常的小工具。<br />把你的想法告诉 AI，让这个项目变成你需要的样子。</p>
    </section>

    <div className="cards">
      <Card><span className="step">01</span><h2>把项目交给 AI</h2><p>回到平台，点击“复制给 AI”。AI 会读取项目说明，编写页面，并构建新的版本。</p></Card>
      <Card><span className="step">02</span><h2>随时打开，继续使用</h2><p>这里是你的初始应用。以后添加的功能也可以在桌面或浏览器使用，AI 关闭后仍能运行，数据保存在本机。</p></Card>
    </div>

    <footer className="footer">
      <span>{tableCount === undefined ? '等待读取项目数据' : tableCount === 0 ? '新项目 · 暂无业务数据表' : `已连接 · ${tableCount} 张业务数据表`}</span>
      <Button className="quiet" onClick={() => void refresh()} disabled={loading}>刷新连接</Button>
    </footer>
  </main>
}
