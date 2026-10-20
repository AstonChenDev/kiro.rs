import { expect, test } from 'bun:test'
import type { Policies } from '../src/api/traffic'
import { buildPolicyPatch as submit, trafficPolicySource } from '../src/lib/traffic-policy'
const policy = (): Policies => ({tiers:{power:{concurrency:10,rpm:20}},defaultConcurrency:0,autoDisableQuota:true,disableAtBaseQuota:true})
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

test('type defaults and subscription edits only send changed leaves', () => {
  const original=policy()
  original.accountTypes={google:{defaults:{concurrency:3,rpm:20},tiers:{max:{concurrency:5,rpm:40}}}}
  const a=structuredClone(original), b=structuredClone(original)
  a.accountTypes!.google!.tiers.max!.concurrency=0
  b.accountTypes!.google!.defaults.rpm=null
  expect(submit(original,a)).toEqual({tiers:{},accountTypes:{google:{tiers:{max:{concurrency:0}}}}})
  expect(submit(original,b)).toEqual({tiers:{},accountTypes:{google:{defaults:{rpm:null}}}})
})
test('new type and unknown subscription are configurable without resetting other policies', () => {
  const original=policy(), draft=structuredClone(original)
  draft.accountTypes={enterprise:{defaults:{concurrency:null,rpm:null},tiers:{unknown:{concurrency:2,rpm:null}}}}
  expect(submit(original,draft)).toEqual({tiers:{},accountTypes:{enterprise:{tiers:{unknown:{concurrency:2}}}}})
  draft.accountTypes.enterprise!.tiers.unknown!.rpm=-1
  expect(()=>submit(original,draft)).toThrow('请输入')
})
test('unchanged empty type policies emit no edits and sources distinguish every layer', () => {
  const original=policy(), draft=structuredClone(original)
  draft.accountTypes={github:{defaults:{concurrency:null,rpm:null},tiers:{}}}
  expect(submit(original,draft)).toEqual({tiers:{}})
  expect(trafficPolicySource('account_type_tier')).toBe('登录类型＋订阅等级')
  expect(trafficPolicySource('account_type')).toBe('登录类型默认')
  expect(trafficPolicySource('tier')).toBe('通用订阅等级')
})
