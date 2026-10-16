import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { normalizeImportAuthMethod, sha256Hex, extractErrorMessage } from '../src/lib/utils'
import { metadataFieldDefault, metadataFieldValues } from '../src/components/credential-metadata-field'
import { normalizeKamAccount } from '../src/lib/kam-import'
import type { AddCredentialRequest } from '../src/types/api'

const fragment = (s:string,a:string,b:string) => s.slice(s.indexOf(a),s.indexOf(b,s.indexOf(a)))
const compile = (s:string) => new Bun.Transpiler({loader:'tsx'}).transformSync(s)
const batch = readFileSync(new URL('../src/components/batch-import-dialog.tsx',import.meta.url),'utf8')
const kam = readFileSync(new URL('../src/components/kam-import-dialog.tsx',import.meta.url),'utf8')
const batchCode = compile(fragment(batch,'  const handleBatchImport =','  const getStatusIcon ='))
const kamCode = compile(fragment(kam,'function normalizeExpiresAt','export function KamImportDialog')+fragment(kam,'  const handleImport =','  const getStatusIcon ='))
const ordinary = 'http://127.0.0.9:3000'
const personal = (provider='Github') => ({refreshToken:'synthetic-proxy-origin-'.repeat(20),authMethod:'social',provider,traffic:{tier:'power',concurrency:15,rpm:10}})
export async function importRequests(kind:'batch'|'kam', input:unknown, pool=true) {
  const captured:AddCredentialRequest[]=[];const errors:string[]=[]
  const args={jsonInput:JSON.stringify([input]),skipErrorAccounts:true,
    existingCredentials:{credentials:[]},proxyPool:{proxies:pool?[{enabled:true,url:ordinary}]:[]},
    normalizeImportAuthMethod,sha256Hex,extractErrorMessage,metadataFieldDefault,metadataFieldValues,normalizeKamAccount,
    setImporting:()=>{},setProgress:()=>{},setCurrentProcessing:()=>{},setResults:()=>{},updateResult:()=>{},
    abortRef:{current:null},queryClient:{invalidateQueries:async()=>{}},
    toast:{error:(s:string)=>errors.push(s),success:()=>{},warning:()=>{},info:()=>{}},
    batchImportCredentials:async(payload:{credentials:AddCredentialRequest[]})=>{captured.push(...payload.credentials)},
  }
  await new Function(...Object.keys(args),`${kind==='batch'?batchCode:kamCode};return ${kind==='batch'?'handleBatchImport':'handleImport'}`)(...Object.values(args))(false)
  return {captured,errors}
}
for(const provider of ['Github',' Google ','BuilderID']) test(`ordinary batch preserves provider ${provider}`,async()=>{
 const input={...personal(provider),...(provider==='BuilderID'?{clientId:'client',clientSecret:'secret'}:{})}
 const {captured,errors}=await importRequests('batch',input)
 expect(errors).toEqual([]);expect(captured[0].provider).toBe(provider.trim())
 expect(captured[0].proxyUrl).toBe(ordinary);expect(captured[0].proxyAutoAssigned).toBe(true)
 expect(captured[0].traffic).toEqual(input.traffic)
})
for(const kind of ['batch','kam'] as const) {
 test(`${kind} marks only automatic pool selections as fallback`,async()=>{
  const {captured,errors}=await importRequests(kind,personal())
  expect(errors).toEqual([]);expect(captured[0]).toMatchObject({provider:'Github',proxyUrl:ordinary,proxyAutoAssigned:true})
  const empty=await importRequests(kind,personal(),false)
  expect(empty.errors).toEqual([]);expect(empty.captured[0].proxyUrl).toBeUndefined()
  expect(empty.captured[0].proxyAutoAssigned).toBe(false)
 })
}
for(const proxyUrl of ['direct','http://127.0.0.8:3000']) test(`explicit ${proxyUrl} and its authentication stay explicit`,async()=>{
 const {captured}=await importRequests('batch',{...personal(),proxyUrl,proxyUsername:'user',proxyPassword:'pass'})
 expect(captured[0]).toMatchObject({proxyUrl,proxyUsername:'user',proxyPassword:'pass',proxyAutoAssigned:false})
})
test('KAM nested identities preserve provider and traffic while marking a pool fallback',async()=>{
 const {captured,errors}=await importRequests('kam',{idp:'Google',credentials:{refreshToken:personal().refreshToken,authMethod:'social'},traffic:{rpm:0}})
 expect(errors).toEqual([]);expect(captured[0]).toMatchObject({provider:'Google',proxyAutoAssigned:true,traffic:{rpm:0}})
})
test('API keys and unknown identities keep their original method and shared pool fallback',async()=>{
 const a=await importRequests('batch',{kiroApiKey:'fixture-key',authMethod:'api_key',provider:'Github'})
 expect(a.captured[0]).toMatchObject({authMethod:'api_key',proxyUrl:ordinary,proxyAutoAssigned:true})
 const b=await importRequests('batch',{refreshToken:personal().refreshToken})
 expect(b.captured[0].provider).toBeUndefined();expect(b.captured[0].proxyUrl).toBe(ordinary)
})

for (const flat of [false,true]) for (const pool of [false,true]) {
 test(`KAM preserves explicitly unknown exported provider (flat=${flat}, pool=${pool})`,async()=>{
  const credentials={refreshToken:personal().refreshToken,authMethod:'social',provider:'Unknown'}
  const input=flat?{...credentials,idp:'Google',traffic:{rpm:0}}:{idp:'Google',credentials,traffic:{rpm:0}}
  const {captured,errors}=await importRequests('kam',input,pool)
  expect(errors).toEqual([]);expect(captured).toHaveLength(1)
  expect(captured[0].provider).toBe('Unknown')
  expect(captured[0].traffic).toEqual({rpm:0})
  expect(captured[0].proxyUrl).toBe(pool?ordinary:undefined)
  expect(captured[0].proxyAutoAssigned).toBe(pool)
 })
}
test('KAM keeps explicit providers and external idp compatibility',async()=>{
 for (const flat of [false,true]) for (const provider of ['Github','Google','BuilderID',undefined,null]) {
  const method=provider==='BuilderID'?'idc':'social'
  const credentials={refreshToken:personal().refreshToken,authMethod:method,provider,
   ...(method==='idc'?{clientId:'fixture-client',clientSecret:'fixture-secret'}:{})}
  const input=flat?{...credentials,idp:'Google'}:{idp:'Google',credentials}
  const {captured,errors}=await importRequests('kam',input,false)
  expect(errors).toEqual([]);expect(captured[0].provider).toBe(provider??'Google')
 }
})
