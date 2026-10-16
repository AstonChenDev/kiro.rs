import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import type { Policies, PolicyPatch } from '../src/api/traffic'
const source = readFileSync(new URL('../src/components/traffic-controls.tsx', import.meta.url), 'utf8')
const start = source.indexOf('          if (!valid(draft.defaultConcurrency)')
const end = source.indexOf('\n        }}>{save.isPending', start)
const code = new Bun.Transpiler({loader:'tsx'}).transformSync(source.slice(start,end))
const TIERS = ['free','pro','pro_plus','max','power','student'] as const
const policy = (): Policies => ({tiers:{power:{concurrency:10,rpm:20}},defaultConcurrency:0,autoDisableQuota:true,disableAtBaseQuota:true})
function submit(original: Policies, draft: Policies) {
  let result: PolicyPatch | undefined
  const args = { original, draft, TIERS,
    valid: (v: number|null) => v === null || (Number.isInteger(v) && v >= 0 && v <= 100000),
    save:{mutate:(patch:PolicyPatch)=>{result=patch}},toast:{error:(m:string)=>{throw Error(m)}} }
  new Function(...Object.keys(args),code)(...Object.values(args))
  return result!
}
test('two editors send only their separately changed concurrency and RPM fields', () => {
  const original=policy(), a=structuredClone(original), b=structuredClone(original)
  a.tiers.power!.concurrency=30; b.tiers.power!.rpm=40
  expect(submit(original,a)).toEqual({tiers:{power:{concurrency:30}}})
  expect(submit(original,b)).toEqual({tiers:{power:{rpm:40}}})
})
test('clearing a field sends explicit null, unlimited sends zero, and the other field is omitted', () => {
  const original=policy(), a=structuredClone(original), b=structuredClone(original)
  a.tiers.power!.concurrency=null; b.tiers.power!.rpm=0
  expect(submit(original,a)).toEqual({tiers:{power:{concurrency:null}}})
  expect(submit(original,b)).toEqual({tiers:{power:{rpm:0}}})
})
test('unchanged inherited fields do not get written alongside other edits', () => {
  const original=policy(), draft=structuredClone(original)
  draft.tiers.free={concurrency:null,rpm:null};draft.autoDisableQuota=false
  expect(submit(original,draft)).toEqual({tiers:{},autoDisableQuota:false})
})
test('invalid fields are rejected before any mutation is sent', () => {
  const original=policy(),draft=structuredClone(original);draft.tiers.power!.rpm=100001
  expect(()=>submit(original,draft)).toThrow('请输入')
})
