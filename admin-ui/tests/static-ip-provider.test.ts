import { expect, test } from 'bun:test'
import { eligibleForStaticIp } from '../src/lib/static-ip-provider'
test('personal provider policy uses provider rather than subscription level', () => {
  expect(eligibleForStaticIp({authMethod:'social',provider:'Github'}, ['github'])).toBe(true)
  expect(eligibleForStaticIp({authMethod:'social',provider:'Google'}, ['github'])).toBe(false)
  expect(eligibleForStaticIp({authMethod:'idc',provider:'BuilderID'}, ['builderid'])).toBe(true)
})
test('enterprise and ambiguous identities cannot enter personal bindings', () => {
  for (const c of [{authMethod:'idc',provider:'Enterprise'}, {authMethod:'external_idp',provider:'Google'}, {authMethod:'api_key',provider:'Github'}, {authMethod:'social',provider:null}]) {
    expect(eligibleForStaticIp(c, ['github','google','builderid'])).toBe(false)
  }
})
