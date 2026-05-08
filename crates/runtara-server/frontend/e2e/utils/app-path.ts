function normalizeBasePath(raw: string | undefined): string {
  if (!raw || raw === '/') return '';
  return `/${raw.replace(/^\/+|\/+$/g, '')}`;
}

function normalizePath(path: string): string {
  return path.startsWith('/') ? path : `/${path}`;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function appPath(path: string): string {
  return `${normalizeBasePath(process.env.E2E_UI_BASE_PATH)}${normalizePath(path)}`;
}

export function appPathPattern(path: string): RegExp {
  return new RegExp(escapeRegExp(appPath(path)));
}

export function appPathExactPattern(path: string): RegExp {
  return new RegExp(`${escapeRegExp(appPath(path))}$`);
}

export function appRoutePattern(routePattern: string): RegExp {
  return new RegExp(
    `${escapeRegExp(normalizeBasePath(process.env.E2E_UI_BASE_PATH))}${normalizePath(routePattern)}`
  );
}
