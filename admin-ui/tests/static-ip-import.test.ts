import { expect, test } from 'bun:test'
import { parseStaticIpLines } from '../src/lib/static-ip-import'

test('vendor table supports spaces and tabs without losing credentials', () => {
  expect(parseStaticIpLines('192.0.2.10\t3000\ttest-user\ttest-password', 'http')).toEqual([
    {host:'192.0.2.10',port:3000,protocol:'http',username:'test-user',password:'test-password'},
  ])
})
test('colon format preserves colons in the password and skips comments', () => {
  const [entry] = parseStaticIpLines('# comment\n192.0.2.10:3000:test-user:p:a:ss\n', 'socks5')
  expect(entry).toMatchObject({host:'192.0.2.10',port:3000,password:'p:a:ss',protocol:'socks5'})
})
test('URL credentials are decoded once and protocol is retained', () => {
  expect(parseStaticIpLines('socks5://u%40x:p%3Ass@192.0.2.10:3000', 'http')[0]).toMatchObject({username:'u@x',password:'p:ss',protocol:'socks5'})
})
test('invalid batch fails as a whole and error does not disclose passwords', () => {
  for (const line of ['192.0.2.10 0 user SECRET', '192.0.2.10 65536 user SECRET', 'ftp://user:SECRET@192.0.2.10:3000', 'http://user:SECRET@192.0.2.10:3000/path']) {
    try { parseStaticIpLines(`192.0.2.11 3000 user pass\n${line}`, 'http'); throw new Error('accepted bad input') }
    catch (e) { expect(String(e)).toContain('第 2 行'); expect(String(e)).not.toContain('SECRET') }
  }
})
test('no-auth proxies and IPv6 URLs are accepted', () => {
  expect(parseStaticIpLines('192.0.2.10:3000', 'http')[0]).toMatchObject({host:'192.0.2.10',port:3000})
  expect(parseStaticIpLines('socks5://[2001:db8::1]:3000', 'http')[0]).toMatchObject({host:'2001:db8::1',port:3000})
})

test('SOCKS URLs without ports use the same default as outbound connections', () => {
  for (const protocol of ['socks5', 'socks5h']) {
    expect(parseStaticIpLines(`${protocol}://user:pass@192.0.2.10`, 'http')[0]).toMatchObject({host:'192.0.2.10',port:1080,protocol})
  }
})


test('URL-looking text inside vendor credentials remains literal', () => {
  for (const line of [
    '192.0.2.10 3000 user p://word',
    '192.0.2.10:3000:user:p://word',
  ]) {
    expect(parseStaticIpLines(line, 'http')[0]).toMatchObject({
      host: '192.0.2.10', port: 3000, username: 'user', password: 'p://word', protocol: 'http',
    })
  }
})

test('bracketed IPv6 vendor endpoints retain port and literal credentials', () => {
  expect(parseStaticIpLines('[2001:db8::1]:3000:user:p:a:ss', 'socks5')[0]).toMatchObject({
    host: '2001:db8::1', port: 3000, username: 'user', password: 'p:a:ss', protocol: 'socks5',
  })
  expect(parseStaticIpLines('[2001:db8::2]:3001', 'http')[0]).toMatchObject({
    host: '2001:db8::2', port: 3001,
  })
})
