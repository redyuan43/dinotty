import type { SshProfile } from '../composables/useSettings'

const SAFE_SESSION_ID = /^[A-Za-z0-9._:-]{1,160}$/

export interface CheckBoardsResumeIntent {
  sessionId: string
  deviceKey: string
  hostname: string
  target: string
  cwd: string
}

export function isValidCheckBoardsResumeIntent(value: unknown): value is CheckBoardsResumeIntent {
  if (!value || typeof value !== 'object') return false
  const intent = value as Partial<CheckBoardsResumeIntent>
  return (
    typeof intent.sessionId === 'string'
    && SAFE_SESSION_ID.test(intent.sessionId.trim())
    && typeof intent.deviceKey === 'string'
    && typeof intent.hostname === 'string'
    && typeof intent.target === 'string'
    && typeof intent.cwd === 'string'
  )
}

export function parseCheckBoardsResumeIntent(search: string): CheckBoardsResumeIntent | null {
  const params = new URLSearchParams(search)
  if (params.get('check_boards_resume') !== '1') return null
  const sessionId = (params.get('session_id') || '').trim()
  if (!SAFE_SESSION_ID.test(sessionId)) return null
  const intent = {
    sessionId,
    deviceKey: (params.get('device_key') || '').trim(),
    hostname: (params.get('hostname') || '').trim(),
    target: (params.get('target') || '').trim(),
    cwd: (params.get('cwd') || '').trim(),
  }
  return isValidCheckBoardsResumeIntent(intent) ? intent : null
}

export function resumeCommandFor(intent: CheckBoardsResumeIntent): string {
  return `siyuan resume ${intent.sessionId}`
}

function normalized(value: string | null | undefined): string {
  return String(value || '').trim().toLowerCase()
}

function profileIdentities(profile: SshProfile): string[] {
  return [
    normalized(profile.name).replace(/^tailscale\s*-\s*/, ''),
    normalized(profile.host),
  ]
}

function targetParts(target: string): { username: string; host: string; port: number | null } {
  let value = target.trim()
  let username = ''
  const at = value.lastIndexOf('@')
  if (at >= 0) {
    username = value.slice(0, at).trim().toLowerCase()
    value = value.slice(at + 1)
  }
  let port: number | null = null
  const portMatch = value.match(/:(\d+)$/)
  if (portMatch) {
    port = Number(portMatch[1])
    value = value.slice(0, -portMatch[0].length)
  }
  return { username, host: value.trim().toLowerCase(), port }
}

export function matchingSshProfiles(profiles: SshProfile[], intent: CheckBoardsResumeIntent): SshProfile[] {
  const target = targetParts(intent.target)
  const sameUser = (items: SshProfile[]) => (
    target.username
      ? items.filter((profile) => normalized(profile.username) === target.username)
      : items
  )
  const samePort = (items: SshProfile[]) => (
    target.port
      ? items.filter((profile) => profile.port === target.port)
      : items
  )
  const exactHost = target.host
    ? profiles.filter((profile) => normalized(profile.host) === target.host)
    : []
  if (exactHost.length) {
    const exact = samePort(sameUser(exactHost))
    return exact.length ? exact : []
  }

  const identityHints = [intent.deviceKey, intent.hostname].map(normalized).filter(Boolean)
  const deviceMatches = identityHints.length
    ? profiles.filter((profile) => {
      const identities = profileIdentities(profile)
      return identityHints.some((hint) => identities.includes(hint))
    })
    : []
  const exact = samePort(sameUser(deviceMatches))
  return exact.length ? exact : deviceMatches
}

export function matchingTerminalTab<T extends { connectionId?: string; type?: string }>(tabs: T[], profileId: string): T | null {
  return tabs.find((tab) => tab.type === 'terminal' && tab.connectionId === profileId) || null
}
