import { api } from './credentials'
export interface StaticIpInput {
  host: string; port: number; protocol: 'http' | 'https' | 'socks5' | 'socks5h'
  username?: string; password?: string; label?: string
}
export interface StaticIpEntry {
  id: number; url: string; username: string | null; hasPassword: boolean; label: string | null
  enabled: boolean; status: 'releasing' | 'free' | 'occupied' | 'reserved' | 'disabled' | 'conflict' | 'global'; credentialIds: number[]
}
export interface StaticIpPool { autoAssign: boolean; allowedProviders: string[]; waitingCredentialIds: number[]; entries: StaticIpEntry[] }
export const getStaticIps = async () => (await api.get<StaticIpPool>('/static-ips')).data
export const importStaticIps = async (entries: StaticIpInput[]) => (await api.post('/static-ips', { entries })).data
export const setStaticIpAutoAssign = async (autoAssign: boolean) => (await api.put('/static-ips/config', { autoAssign })).data
export const setStaticIpEnabled = async (id: number, enabled: boolean) => (await api.put(`/static-ips/${id}/enabled`, { enabled })).data
export const deleteStaticIp = async (id: number) => (await api.delete(`/static-ips/${id}`)).data
export const deleteStaticIps = async (ids: number[]) => (await api.post('/static-ips/batch-delete', { ids })).data
export const bindStaticIp = async (credentialId: number, ipId: number | null) =>
  (await api.put(`/credentials/${credentialId}/static-ip`, { ipId })).data

export const setStaticIpProviders = async (allowedProviders: string[]) => (await api.put('/static-ips/config', { allowedProviders })).data
