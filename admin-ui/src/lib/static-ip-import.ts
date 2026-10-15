import type { StaticIpInput } from '@/api/static-ips'

/** Four whitespace-separated vendor columns, host:port:user:password, or a full proxy URL. */
export function parseStaticIpLines(text: string, protocol: StaticIpInput['protocol']): StaticIpInput[] {
  const lines = text.split(/\r?\n/).map(s => s.trim()).filter(s => s && !s.startsWith('#'))
  if (!lines.length || lines.length > 1000) throw new Error('每次请填写 1～1000 条静态 IP')
  return lines.map((line, i) => {
    try {
      let entry: StaticIpInput
      // Only a leading scheme selects URL syntax; vendor passwords are literal.
      if (/^[a-z][a-z0-9+.-]*:\/\//i.test(line)) {
        const url = new URL(line)
        if (!['http:', 'https:', 'socks5:', 'socks5h:'].includes(url.protocol) || url.search || url.hash || (url.pathname && url.pathname !== '/')) throw new Error()
        entry = { host: url.hostname.replace(/^\[|\]$/g, ''), port: Number(url.port || (url.protocol === 'http:' ? 80 : url.protocol === 'https:' ? 443 : 1080)),
          protocol: url.protocol.slice(0, -1) as StaticIpInput['protocol'], username: decodeURIComponent(url.username), password: decodeURIComponent(url.password) }
      } else {
        let parts = line.split(/\s+/)
        if (parts.length === 1) {
          const match = line.match(/^(\[[^\]]+\]|[^:]+):(\d+)(?::([^:]*):(.*))?$/)
          if (!match) throw new Error()
          parts = [match[1].replace(/^\[|\]$/g, ''), match[2], match[3] || '', match[4] || '']
        }
        if (parts.length !== 2 && parts.length !== 4) throw new Error()
        entry = { host: parts[0], port: Number(parts[1]), protocol, username: parts[2], password: parts[3] }
      }
      if (!entry.host || !Number.isInteger(entry.port) || entry.port < 1 || entry.port > 65535) throw new Error()
      return entry
    } catch { throw new Error(`第 ${i + 1} 行格式不正确，请使用 IP 端口 用户名 密码，或完整代理 URL`) }
  })
}
