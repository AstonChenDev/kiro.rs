// Match the server's conservative personal-identity classification.
export function eligibleForStaticIp(c: { authMethod?: string | null; provider?: string | null }, allowed: string[]) {
  const method = (c.authMethod ?? '').trim().toLowerCase()
  const provider = (c.provider ?? '').trim().toLowerCase()
  if (['social', ''].includes(method) && ['github', 'google'].includes(provider)) return allowed.includes(provider)
  if (['idc', 'builder-id', ''].includes(method) && ['builderid', 'builder-id', 'builder_id'].includes(provider)) return allowed.includes('builderid')
  return false
}
