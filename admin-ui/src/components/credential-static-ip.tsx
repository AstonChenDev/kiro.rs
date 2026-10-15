import { useState } from 'react'
import { isAxiosError } from 'axios'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { bindStaticIp, getStaticIps, type StaticIpEntry } from '@/api/static-ips'
import { Button } from '@/components/ui/button'
import { eligibleForStaticIp } from '@/lib/static-ip-provider'
import { extractErrorMessage } from '@/lib/utils'
import type { CredentialStatusItem } from '@/types/api'

const statusLabels: Record<StaticIpEntry['status'], string> = {
  free: '空闲', occupied: '已占用', releasing: '释放中', reserved: '分配中',
  disabled: '已暂停分配', conflict: '绑定冲突', global: '全局代理占用',
}

export function CredentialStaticIp({ credential }: { credential: CredentialStatusItem }) {
  const client = useQueryClient()
  const pool = useQuery({ queryKey: ['static-ips'], queryFn: getStaticIps, refetchInterval: 3000 })
  const [selected, setSelected] = useState<string | null>(null)
  const entries = pool.data?.entries ?? []
  const current = entries.find(ip => ip.credentialIds.includes(credential.id))
  const value = selected ?? (current ? String(current.id) : '')
  const target = entries.find(ip => String(ip.id) === value)
  const allowed = eligibleForStaticIp(credential, pool.data?.allowedProviders ?? [])
  const save = useMutation({
    mutationFn: (ipId: number) => bindStaticIp(credential.id, ipId),
    onSuccess: () => { toast.success('静态 IP 已绑定到该账号') },
    onError: async (error, ipId) => {
      const status = isAxiosError(error) ? error.response?.status : undefined
      if (status && status >= 400 && status < 500 && status !== 408) {
        toast.error(`绑定失败：${extractErrorMessage(error)}`)
        return
      }
      // A lost response does not tell us whether the write committed. Only a
      // fresh successful read can confirm the current binding, never cached data.
      try {
        const verified = await pool.refetch({ cancelRefetch: true })
        if (!verified.isError && verified.data) {
          const ip = verified.data.entries.find(entry => entry.id === ipId)
          if (ip?.status === 'occupied' && ip.credentialIds.length === 1 && ip.credentialIds[0] === credential.id) {
            setSelected(null)
            toast.success('已核实：该账号当前已绑定所选静态 IP')
          } else {
            toast.warning('保存响应异常，当前未确认所选 IP 的独占绑定，请核对当前状态后重试')
          }
          return
        }
      } catch {
        // A cancelled or failed verification leaves the write outcome unknown.
      }
      toast.warning('未能确认保存结果，请恢复连接后核对当前绑定')
    },
    onSettled: async (_data, error) => {
      await Promise.all(['static-ips', 'credentials', 'proxy-pool', 'credential-traffic'].map(key =>
        client.invalidateQueries({ queryKey: [key] }),
      ))
      if (!error) setSelected(null)
    },
  })
  const disabled = save.isPending || credential.disabled || !allowed || !pool.data || pool.isError

  return <div className="space-y-2 border-t border-border/30 pt-2 mt-2" data-no-rect-select>
    <label htmlFor={`credential-static-ip-${credential.id}`} className="text-xs font-medium">固定静态 IP</label>
    <p className="text-[11px] text-muted-foreground break-all">
      {current ? `当前绑定：${current.url}（${statusLabels[current.status]}）` : '当前未绑定静态 IP'}
    </p>
    <div className="flex min-w-0 gap-2">
      <select id={`credential-static-ip-${credential.id}`} className="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-xs"
        value={value} disabled={disabled} onChange={event => setSelected(event.target.value)}>
        <option value="">选择空闲静态 IP</option>
        {selected && !target && <option value={selected} disabled>所选 IP 已不可用，请重新选择</option>}
        {entries.map(ip => <option key={ip.id} value={ip.id} disabled={!ip.enabled || ip.status !== 'free'}>
          {ip.url} · {ip.id === current?.id ? '当前绑定 · ' : ''}{statusLabels[ip.status]}{ip.label ? ` · ${ip.label}` : ''}
        </option>)}
      </select>
      <Button type="button" size="sm" variant="outline" className="h-8 shrink-0 text-xs"
        disabled={disabled || !target?.enabled || target.status !== 'free' || target.id === current?.id}
        onClick={() => { if (target) save.mutate(target.id) }}>
        {save.isPending ? '保存中…' : '保存'}
      </Button>
    </div>
    <p className="text-[11px] text-muted-foreground">
      {pool.isError ? '静态 IP 列表加载失败，请重试。'
        : !pool.data ? '正在加载静态 IP…'
        : credential.disabled ? '账号已禁用，恢复启用后可绑定。'
        : !allowed ? '当前账号类型不在静态 IP 允许范围内。'
        : !entries.some(ip => ip.enabled && ip.status === 'free') ? '暂无空闲 IP，可在静态 IP 管理中添加。'
        : '保存后使用所选 IP 及其认证信息；更换会释放原静态 IP。有在途请求时请稍后重试。'}
    </p>
    {pool.isError && <Button type="button" size="sm" variant="ghost" disabled={pool.isFetching}
      onClick={() => { void pool.refetch() }}>重新加载</Button>}
  </div>
}
