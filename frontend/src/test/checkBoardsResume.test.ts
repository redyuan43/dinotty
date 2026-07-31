import { describe, expect, it } from 'vitest'
import {
  matchingSshProfiles,
  matchingTerminalTab,
  parseCheckBoardsResumeIntent,
  resumeCommandFor,
  isValidCheckBoardsResumeIntent,
} from '../utils/checkBoardsResume'

describe('Check Boards resume intent', () => {
  it('parses and validates a deep link without accepting a raw command', () => {
    const intent = parseCheckBoardsResumeIntent(
      '?check_boards_resume=1&session_id=019f79d3-f18b-72d1-aab3-b687b40f10d4&device_key=AMD&target=ivan%40amd.example&cwd=%2Fhome%2Fivan'
    )
    expect(intent).toEqual({
      sessionId: '019f79d3-f18b-72d1-aab3-b687b40f10d4',
      deviceKey: 'AMD',
      target: 'ivan@amd.example',
      cwd: '/home/ivan',
    })
    expect(resumeCommandFor(intent!)).toBe('siyuan resume 019f79d3-f18b-72d1-aab3-b687b40f10d4')
    expect(parseCheckBoardsResumeIntent('?check_boards_resume=1&session_id=bad%3Bshutdown')).toBeNull()
    expect(isValidCheckBoardsResumeIntent({
      sessionId: 'bad\rwhoami',
      deviceKey: 'AMD',
      target: '',
      cwd: '',
    })).toBe(false)
  })

  it('selects the strongest saved SSH profile match', () => {
    const profiles = [
      { id: 'wrong', name: 'AMD', host: 'other.example', port: 22, username: 'ivan', auth_method: { type: 'key_file', key_path: '/tmp/key' } },
      { id: 'right', name: 'SuperAI', host: 'amd.example', port: 22, username: 'ivan', auth_method: { type: 'key_file', key_path: '/tmp/key' } },
    ] as any
    const intent = parseCheckBoardsResumeIntent('?check_boards_resume=1&session_id=session-root-1&device_key=AMD&target=ivan%40amd.example')!
    expect(matchingSshProfiles(profiles, intent).map((profile) => profile.id)).toEqual(['right'])
  })

  it('reuses an existing terminal connected by profile id', () => {
    const tabs = [
      { type: 'terminal', paneId: 'tab-1', connectionId: 'profile-1' },
      { type: 'terminal', paneId: 'tab-2', connectionId: 'profile-2' },
    ]
    expect(matchingTerminalTab(tabs, 'profile-2')?.paneId).toBe('tab-2')
  })

  it('rejects an exact-host profile with the wrong username instead of guessing', () => {
    const profiles = [
      { id: 'wrong-user', name: 'Other', host: 'amd.example', port: 22, username: 'root', auth_method: { type: 'key_file', key_path: '/tmp/key' } },
      { id: 'alias', name: 'AMD', host: 'amd-alias', port: 22, username: 'ivan', auth_method: { type: 'key_file', key_path: '/tmp/key' } },
    ] as any
    const intent = parseCheckBoardsResumeIntent('?check_boards_resume=1&session_id=session-root-1&device_key=AMD&target=ivan%40amd.example')!
    expect(matchingSshProfiles(profiles, intent)).toEqual([])
  })
})
