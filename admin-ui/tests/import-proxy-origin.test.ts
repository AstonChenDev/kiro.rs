import { parseCredentialImport, importedSubscriptionTitle, normalizeImportExpiresAt } from '../src/lib/credential-import'
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
export async function importRequests(kind:'batch'|'kam', input:unknown, pool=true, raw=false) {
  const captured:AddCredentialRequest[]=[];const errors:string[]=[]
  const args={jsonInput:JSON.stringify(raw ? input : [input]),skipErrorAccounts:true,
    existingCredentials:{credentials:[]},proxyPool:{proxies:pool?[{enabled:true,url:ordinary}]:[]},
    parseCredentialImport,importedSubscriptionTitle,normalizeImportExpiresAt,normalizeImportAuthMethod,sha256Hex,extractErrorMessage,metadataFieldDefault,metadataFieldValues,normalizeKamAccount,
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

test('ordinary batch imports all five nested KAM personal accounts without losing provider', async () => {
  const accounts = Array.from({length:5}, (_, index) => ({
    idp:'Github', email:`fixture-${index}@example.invalid`, machineId:`fixture-machine-${index}`,
    credentials:{refreshToken:`fixture-${index}-`.repeat(30),authMethod:'social',provider:'Github',clientId:'',clientSecret:'',region:'us-east-1'},
  }))
  const {captured,errors}=await importRequests('batch',{version:'1.5.0',accounts,groups:[],tags:[]},false,true)
  expect(errors).toEqual([])
  expect(captured).toHaveLength(5)
  captured.forEach((request,index)=>{
    expect(request).toMatchObject({refreshToken:accounts[index].credentials.refreshToken,
      authMethod:'social',provider:'Github',email:accounts[index].email,machineId:accounts[index].machineId})
    expect(request.proxyUrl).toBeUndefined()
  })
})
test('nested KAM input accepts idp fallback in the common batch entry', async () => {
  const {captured}=await importRequests('batch',{idp:'Google',credentials:{refreshToken:personal().refreshToken,authMethod:'social'}},false)
  expect(captured[0]).toMatchObject({authMethod:'social',provider:'Google'})
})
test('malformed containers fail clearly before sending an import request', () => {
  for (const input of [null, {accounts:{}}, {accounts:[null]}, {credentials:[]}]) {
    expect(()=>parseCredentialImport(JSON.stringify(input))).toThrow()
  }
  expect(parseCredentialImport('[]')).toEqual([])
})

for (const kind of ['batch','kam'] as const) test(`${kind} preserves Pro+ and access-token metadata without forcing a traffic override`, async () => {
  const {captured,errors}=await importRequests(kind,{email:'fixture@example.invalid',idp:'Github',
    subscription:{title:'KIRO PRO+',type:'Pro_Plus'},credentials:{refreshToken:personal().refreshToken,
    accessToken:'fixture-access',expiresAt:1793491200000,profileArn:'fixture-profile',authMethod:'social',provider:'Github'}},false)
  expect(errors).toEqual([])
  expect(captured[0]).toMatchObject({subscriptionTitle:'KIRO PRO+',accessToken:'fixture-access',profileArn:'fixture-profile',expiresAt:new Date(1793491200000).toISOString()})
  expect(captured[0].traffic?.tier).toBeUndefined()
})
test('flat KAM and exported credentials preserve title, unknown plans stay unknown', async () => {
  expect(importedSubscriptionTitle({subscription:{type:'Pro_Plus'}})).toBe('KIRO PRO+')
  expect(importedSubscriptionTitle({subscription:{type:'unrecognized'}})).toBeUndefined()
  for(const kind of ['batch','kam'] as const){
    const {captured}=await importRequests(kind,{...personal(),subscriptionTitle:'KIRO PRO+',accessToken:'fixture-access'},false)
    expect(captured[0].subscriptionTitle).toBe('KIRO PRO+')
  }
})

test('five comma-separated Google objects preserve tokens and infer social login', async () => {
 const accounts=Array.from({length:5},(_,i)=>({email:`google-${i}@example.invalid`,provider:'Google',refreshToken:`fixture-${i}:+/`.repeat(30)}))
 const pasted=accounts.map(a=>JSON.stringify(a)).join(',\n')
 const parsed=parseCredentialImport(pasted)
 expect(parsed).toHaveLength(5)
 const {captured,errors}=await importRequests('batch',parsed,false,true)
 expect(errors).toEqual([]);expect(captured).toHaveLength(5)
 captured.forEach((r,i)=>expect(r).toMatchObject({...accounts[i],authMethod:'social'}))
})
test('malformed JSON never echoes credential values and never evaluates text', () => {
 for (const raw of ['{"refreshToken":"private-fixture",}', '{"refreshToken":"private-fixture"}, malicious()', '[{"refreshToken":"private-fixture"}']) {
  try {parseCredentialImport(raw);throw Error('unexpected success')}
  catch(error) {expect(String(error)).not.toContain('private-fixture');expect(String(error)).toContain('JSON')}
 }
})
