import { useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { getStaticIps, importStaticIps, setStaticIpAutoAssign, setStaticIpProviders, setStaticIpEnabled, deleteStaticIp, bindStaticIp, type StaticIpInput } from '@/api/static-ips'
import { getCredentials } from '@/api/credentials'
import { eligibleForStaticIp } from '@/lib/static-ip-provider'
import { parseStaticIpLines } from '@/lib/static-ip-import'
import { extractErrorMessage } from '@/lib/utils'

const labels = { releasing: '等待请求结束后释放', free: '空闲', occupied: '已占用', reserved: '分配中', disabled: '暂停分配', conflict: '多个账号占用', global: '全局占用' }
export function StaticIpDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
  const client = useQueryClient()
  const pool = useQuery({ queryKey: ['static-ips'], queryFn: getStaticIps, enabled: open, refetchInterval: open ? 3000 : false })
  const credentials = useQuery({ queryKey: ['credentials'], queryFn: getCredentials, enabled: open })
  const [host, setHost] = useState(''), [port, setPort] = useState('3000')
  const [username, setUsername] = useState(''), [password, setPassword] = useState(''), [label, setLabel] = useState('')
  const [protocol, setProtocol] = useState<StaticIpInput['protocol']>('http')
  const [batch, setBatch] = useState(false), [text, setText] = useState('')
  const [busy, setBusy] = useState(false), lock = useRef(false)
  const recoveryRequired = useRef(false)
  const [needsReload, setNeedsReload] = useState(false)
  const [selected, setSelected] = useState<Record<number, string>>({})
  const [filter, setFilter] = useState('all')
  async function readLatestPool() {
    const result = await pool.refetch({ cancelRefetch: true })
    // React Query can retain old data on a failed refetch. It cannot confirm a write.
    if (result.isError || !result.data) throw result.error ?? new Error('静态 IP 状态读取失败')
    return result.data
  }
  async function synchronizePool() {
    try {
      await readLatestPool()
      recoveryRequired.current = false; setNeedsReload(false)
    } catch {
      recoveryRequired.current = true; setNeedsReload(true)
    }
  }
  async function retryReload() {
    if (lock.current) return
    lock.current = true; setBusy(true)
    try { await synchronizePool() }
    finally { lock.current = false; setBusy(false) }
  }
  async function run(action: () => Promise<unknown>, message: string, clear?: () => void) {
    if (lock.current || recoveryRequired.current) return
    lock.current = true; setBusy(true)
    try {
      try {
        await action(); clear?.(); toast.success(message)
      } catch (e) {
        toast.warning(`操作未正常返回：${extractErrorMessage(e)}；请核对重新加载后的状态`)
      }
      // A write may have committed despite a lost response. Keep edits locked
      // until a fresh read succeeds; background polling does not release this guard.
      await synchronizePool()
      await Promise.all(['credentials', 'proxy-pool'].map(key => client.invalidateQueries({ queryKey: [key] })))
    } finally { lock.current = false; setBusy(false) }
  }
  async function changeProvider(value: string, enabled: boolean) {
    await run(async () => {
      // Build the replacement whitelist from a fresh read, never a render's cache.
      const latest = await readLatestPool()
      await setStaticIpProviders(enabled
        ? Array.from(new Set([...latest.allowedProviders, value]))
        : latest.allowedProviders.filter(p => p !== value))
    }, '账号类型策略已保存')
  }
  function add() {
    try {
      const entries = batch ? parseStaticIpLines(text, protocol) : [{ host: host.trim(), port: Number(port), protocol, username, password, label }]
      if (entries.some(e => !e.host || !Number.isInteger(e.port) || e.port < 1 || e.port > 65535)) throw new Error('请填写有效的 IP 和 1～65535 的端口')
      void run(() => importStaticIps(entries), `已导入 ${entries.length} 个静态 IP`, () => { setHost(''); setUsername(''); setPassword(''); setLabel(''); setText('') })
    } catch (e) { toast.error(extractErrorMessage(e)) }
  }
  const rows = pool.data?.entries ?? []
  const editsDisabled = busy || needsReload || pool.isError || !pool.data
  return <Dialog open={open} onOpenChange={v => { if (!busy) onOpenChange(v) }}>
    <DialogContent className="sm:max-w-4xl max-h-[90vh] overflow-y-auto">
      <DialogHeader><DialogTitle>静态 IP 管理</DialogTitle></DialogHeader>
      <p className="text-sm text-muted-foreground">每个静态 IP 独占绑定一个账号。正常账号保持固定绑定。封禁、超额关闭、失效或手动禁用后自动释放；仍有请求时等待结束再释放。删除账号也会释放。</p>
      {(needsReload || pool.isError) && <div role="alert" className="rounded-lg border border-amber-500/40 p-3 text-sm text-amber-600">
        当前静态 IP 状态尚未核实，已暂停修改。请重新加载并核对状态后继续。
        <Button size="sm" variant="outline" className="ml-2" disabled={busy} onClick={() => void retryReload()}>重新加载状态</Button>
      </div>}
      <div className="rounded-lg border p-3 space-y-2">
        <label className="flex items-center justify-between gap-3"><span className="font-medium text-sm">新导入账号自动分配空闲 IP</span>
          <Switch aria-label="新导入账号自动分配空闲 IP" checked={pool.data?.autoAssign ?? false} disabled={editsDisabled}
            onCheckedChange={v => void run(() => setStaticIpAutoAssign(v), v ? '已开启新账号自动分配' : '已关闭自动分配，已有绑定保留')} /></label>
        <p className="text-xs text-muted-foreground">开启后，允许类型的新账号先正常导入并获取订阅和额度，再独占绑定空闲 IP 并启用；没有空闲 IP 时保留完整账号信息，进入“等待静态 IP”禁用状态。新增、启用或释放 IP 后自动绑定并启用。明确指定普通代理或直连的账号沿用其设置。已有账号不会自动迁移。</p>
      </div>
      <div className="rounded-lg border p-3 space-y-2">
        <p className="font-medium text-sm">允许使用静态 IP 的个人账号类型</p>
        <div className="flex flex-wrap gap-4">{[['github', 'GitHub'], ['google', 'Google'], ['builderid', 'Builder ID']].map(([value, title]) =>
          <label key={value} className="flex items-center gap-2 text-sm"><Switch aria-label={`${title} 使用静态 IP`} checked={pool.data?.allowedProviders.includes(value) ?? false} disabled={editsDisabled}
            onCheckedChange={enabled => void changeProvider(value, enabled)} />{title}</label>)}</div>
        <p className="text-xs text-muted-foreground">默认允许 GitHub、Google。Enterprise、IAM SSO、Entra ID、API Key 和未知类型不分配。关闭某类型会释放该类型已有的静态 IP，账号恢复全局代理或直连；若仍有请求，需等请求结束后再关闭。</p>
        <p className="text-xs text-muted-foreground">异常账号恢复后会重新领取空闲 IP；没有空闲 IP 时暂停调用，不会借用其他账号的地址。关闭自动分配不会取消这项恢复保护。</p>
        {!!pool.data?.waitingCredentialIds.length && <p className="text-sm text-amber-600">等待空闲 IP 的账号：{pool.data.waitingCredentialIds.map(id => `#${id}`).join('、')}</p>}
      </div>
      <div className="flex flex-wrap gap-4 text-sm"><span>总数 {rows.length}</span><span>空闲 {rows.filter(x => x.status === 'free').length}</span><span>已占用 {rows.filter(x => x.credentialIds.length > 0).length}</span><span>分配中 {rows.filter(x => x.status === 'reserved').length}</span></div>
      <fieldset disabled={editsDisabled} className="rounded-lg border p-3 space-y-3">
        <div className="flex gap-2 items-center"><span className="text-sm font-medium">添加静态代理</span><select aria-label="静态代理协议" className="border rounded bg-background p-1.5 text-sm" value={protocol} onChange={e => setProtocol(e.target.value as StaticIpInput['protocol'])}>
          <option value="http">HTTP</option><option value="https">HTTPS</option><option value="socks5">SOCKS5</option><option value="socks5h">SOCKS5H</option>
        </select><Button size="sm" variant="outline" onClick={() => setBatch(!batch)}>{batch ? '单条添加' : '批量导入'}</Button></div>
        <p className="text-xs text-muted-foreground">协议请按供应商要求选择；用户名和密码分别填写，系统自动生成完整代理配置。</p>
        {batch ? <><textarea aria-label="批量静态 IP" className="w-full min-h-32 border rounded bg-background p-2 font-mono text-sm" value={text} onChange={e => setText(e.target.value)} placeholder={'每行：IP 端口 用户名 密码\n192.0.2.10 3000 username password\n也支持 IP:端口:用户名:密码 或完整代理 URL'} /><p className="text-xs text-muted-foreground">支持复制供应商的四列数据，以空格或制表符分隔。整批校验通过才保存，不会部分写入。</p></> :
          <div className="grid grid-cols-2 gap-2"><Input aria-label="静态 IP 地址" placeholder="IP 地址" value={host} onChange={e => setHost(e.target.value)} /><Input aria-label="静态 IP 端口" type="number" min={1} max={65535} placeholder="端口" value={port} onChange={e => setPort(e.target.value)} />
            <Input aria-label="静态代理用户名" autoComplete="off" placeholder="用户名（可选）" value={username} onChange={e => setUsername(e.target.value)} /><Input aria-label="静态代理密码" autoComplete="new-password" type="password" placeholder="密码（可选）" value={password} onChange={e => setPassword(e.target.value)} />
            <Input className="col-span-2" aria-label="静态 IP 备注" placeholder="备注，例如地区、供应商或到期日期" value={label} onChange={e => setLabel(e.target.value)} /></div>}
        <Button size="sm" onClick={add} disabled={busy || (batch ? !text.trim() : !host.trim())}>{busy ? '处理中…' : batch ? '导入静态 IP' : '添加静态 IP'}</Button>
      </fieldset>
      <div className="flex justify-between items-center"><span className="font-medium text-sm">静态 IP 列表</span><select aria-label="静态 IP 状态筛选" className="border rounded bg-background p-1 text-sm" value={filter} onChange={e => setFilter(e.target.value)}><option value="all">全部</option>{Object.entries(labels).map(([value, title]) => <option key={value} value={value}>{title}</option>)}</select></div>
      {pool.isError && <p className="text-sm text-destructive">加载失败：{extractErrorMessage(pool.error)}</p>}
      {pool.isLoading && <p className="text-sm text-muted-foreground">加载中…</p>}
      {!pool.isLoading && !pool.isError && !rows.length && <p className="text-sm text-muted-foreground">尚未添加静态 IP。先添加代理，再开启自动分配。</p>}
      <div className="space-y-2">{rows.filter(ip => filter === 'all' || ip.status === filter).map(ip => <div key={ip.id} className="border rounded-lg p-3 space-y-2">
        <div className="flex flex-wrap items-center justify-between gap-2"><span className="font-mono text-sm">{ip.url}</span><span className={ip.status === 'free' ? 'text-sm text-emerald-600' : 'text-sm'}>{labels[ip.status]}{!ip.enabled && ip.status !== 'disabled' ? ' · 暂停新分配' : ''}</span></div>
        <p className="text-xs text-muted-foreground">{ip.label ? `${ip.label} · ` : ''}{ip.username ? `用户名：${ip.username}` : '无用户名'} · 密码{ip.hasPassword ? '已配置' : '未配置'}</p>
        {ip.credentialIds.map(id => <div key={id} className="flex gap-2 justify-between items-center text-sm"><span>账号 #{id} {credentials.data?.credentials.find(c => c.id === id)?.email ?? ''}</span><Button size="sm" variant="outline" disabled={editsDisabled} onClick={() => { if (window.confirm('解除后，该账号会恢复原来的全局代理或直连。确定解除静态 IP 绑定？')) void run(() => bindStaticIp(id, null), '已解除绑定，IP 可重新分配') }}>解除绑定</Button></div>)}
        {ip.status === 'free' && <div className="flex gap-2"><select aria-label={`静态 IP ${ip.id} 绑定账号`} className="border rounded bg-background p-1 text-sm min-w-0 flex-1" value={selected[ip.id] ?? ''} disabled={editsDisabled} onChange={e => setSelected({ ...selected, [ip.id]: e.target.value })}>
          <option value="">选择现有账号（可选）</option>{credentials.data?.credentials.filter(c => !c.disabled && eligibleForStaticIp(c, pool.data?.allowedProviders ?? [])).map(c => <option key={c.id} value={c.id}>#{c.id} {c.email ?? ''}</option>)}</select>
          <Button size="sm" variant="outline" disabled={editsDisabled || !selected[ip.id]} onClick={() => void run(() => bindStaticIp(Number(selected[ip.id]), ip.id), '静态 IP 已绑定到账号')}>绑定账号</Button></div>}
        <div className="flex gap-2"><Button size="sm" variant="ghost" disabled={editsDisabled || ip.status === 'reserved'} onClick={() => void run(() => setStaticIpEnabled(ip.id, !ip.enabled), ip.enabled ? '已暂停新分配，已有绑定保留' : '已允许分配')}>{ip.enabled ? '暂停分配' : '允许分配'}</Button>
          <Button size="sm" variant="ghost" className="text-destructive" disabled={editsDisabled || !['free', 'disabled'].includes(ip.status)} onClick={() => { if (window.confirm('确定从静态 IP 库删除这条空闲代理？')) void run(() => deleteStaticIp(ip.id), '静态 IP 已删除') }}>删除</Button></div>
      </div>)}</div>
    </DialogContent>
  </Dialog>
}
