import { toast } from 'sonner';
import { config } from '@/shared/config/runtimeConfig';

const parse = (version: string | undefined) => {
  const match = /^v?(\d+)\.(\d+)\.(\d+)/.exec(version?.trim() ?? '');
  return match ? { major: Number(match[1]), minor: Number(match[2]) } : null;
};

/**
 * Whether a console of `uiVersion` works against a server of `serverVersion`:
 * the same major, and minors at most one apart (each side keeps the API of
 * the previous minor working while a release rolls out). `null` when either
 * isn't a release version (`dev`, a bare commit), so nothing can be said.
 */
export function isCompatibleServer(
  uiVersion: string | undefined,
  serverVersion: string | undefined
): boolean | null {
  const ui = parse(uiVersion);
  const server = parse(serverVersion);
  if (!ui || !server) return null;
  return ui.major === server.major && Math.abs(ui.minor - server.minor) <= 1;
}

let warned = false;

/**
 * Warn once when a statically hosted console talks to a server release it was
 * not built for. Served by runtara-server, the console is that server's own
 * build, so there is nothing to check.
 */
export function checkServerVersion(serverVersion: string | undefined) {
  if (config.hosting !== 'static' || warned) return;
  if (isCompatibleServer(config.build.version, serverVersion) !== false) return;
  warned = true;
  console.warn(
    `[runtara] console ${config.build.version} does not support server ${serverVersion}`
  );
  toast.warning('This console was not built for this server', {
    description: `Console ${config.build.version}, server ${serverVersion}. Some pages may not work.`,
    duration: Infinity,
  });
}
