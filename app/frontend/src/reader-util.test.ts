import { describe, expect, it } from 'vitest'
import { fmt } from './reader-util'

describe('fmt', () => {
  it('formats zero', () => {
    expect(fmt(0)).toBe('0:00')
  })

  it('pads seconds under 10', () => {
    expect(fmt(5)).toBe('0:05')
  })

  it('formats whole minutes', () => {
    expect(fmt(60)).toBe('1:00')
  })

  it('formats minutes and seconds', () => {
    expect(fmt(65)).toBe('1:05')
  })

  it('formats durations over an hour as minutes, not hours', () => {
    expect(fmt(3661)).toBe('61:01')
  })

  it('truncates fractional seconds rather than rounding', () => {
    expect(fmt(59.9)).toBe('0:59')
  })
})
