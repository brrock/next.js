export type PackageManager = 'npm' | 'pnpm' | 'yarn' | 'bun' | 'deno'

export function getPkgManager(): PackageManager {
  // Check if Deno exists in the global namespace
  if (typeof globalThis.Deno !== 'undefined') {
    return 'deno'
  }

  const userAgent = process.env.npm_config_user_agent || ''

  if (userAgent.startsWith('yarn')) {
    return 'yarn'
  }

  if (userAgent.startsWith('pnpm')) {
    return 'pnpm'
  }

  if (userAgent.startsWith('bun')) {
    return 'bun'
  }

  return 'npm'
}