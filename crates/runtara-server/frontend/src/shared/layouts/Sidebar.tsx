import { ComponentPropsWithoutRef, useMemo } from 'react';
import { Link, useLocation, useSearchParams } from 'react-router';
import {
  Sidebar as SidebarPrimitive,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarTrigger,
  useSidebar,
} from '@/shared/components/ui/sidebar.tsx';
import { menu, filterMenu, type MenuItem } from '@/shared/config';
import { config } from '@/shared/config/runtimeConfig';
import { useEntitlements } from '@/shared/hooks/useEntitlements';
import Logo from '@/assets/logo/runtara-logo-icon.svg';
import { AuthSidebar } from './AuthSidebar.tsx';
import { TenantSwitcher } from '@/shared/tenant/TenantSwitcher';
import {
  useAuthStore,
  useHasExplicitPermission,
} from '@/shared/stores/authStore.ts';
import { ThemeSwitcher } from '@/shared/components/theme-switcher.tsx';
import { Settings } from 'lucide-react';
import { cn } from '@/lib/utils';
import { edition } from '@edition';

export function Sidebar() {
  return (
    <SidebarPrimitive collapsible="icon" variant="sidebar">
      <SidebarHeader>
        <HeaderMenu />
      </SidebarHeader>
      <SidebarContent>
        <AppMenu />
      </SidebarContent>
      <SidebarFooter>
        <FooterMenu />
      </SidebarFooter>
    </SidebarPrimitive>
  );
}

function HeaderMenu() {
  return (
    <SidebarMenuButton size="lg" asChild>
      <Link className="flex items-center gap-2" to="/">
        <div className="flex aspect-square size-8 items-center justify-center rounded-sm">
          <img src={Logo} alt="Runtara logo" />
        </div>
        <div className="grid flex-1 text-left leading-tight group-data-[collapsible=icon]:hidden">
          <span className="truncate text-lg font-semibold text-sidebar-foreground">
            Runtara
          </span>
          <span
            role="link"
            tabIndex={0}
            className="cursor-pointer text-2xs text-muted-foreground no-underline hover:text-foreground"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              window.open(
                'https://runtara.com',
                '_blank',
                'noopener,noreferrer'
              );
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                e.stopPropagation();
                window.open(
                  'https://runtara.com',
                  '_blank',
                  'noopener,noreferrer'
                );
              }
            }}
          ></span>
        </div>
      </Link>
    </SidebarMenuButton>
  );
}

function AppMenu() {
  const location = useLocation();

  const userGroups = useAuthStore((state) => state.userGroups);
  const entitlements = useEntitlements();

  const allowedMenu = useMemo(
    () => filterMenu([...menu, ...edition.menu], userGroups, entitlements),
    [userGroups, entitlements]
  );

  // Settings is a collapsible group whose children are the settings sections.
  // "User Management" links out to the separate smo-management SPA
  // (/ui/management/) and is default-deny: shown only when /me explicitly grants
  // `user_management:access` (the target surface exists only in the managed
  // deployment, so it must stay hidden rather than render a dead link in
  // self-hosted modes). UI-only gate; smo-management enforces.
  const canManageUsers = useHasExplicitPermission('user_management:access');

  const settingsItem: MenuItem = useMemo(
    () => ({
      key: 'settings',
      title: 'Settings',
      to: '/settings',
      icon: <Settings size={16} />,
      allowedGroups: [],
      children: [
        { key: 'api-keys', title: 'API Keys', to: '/settings/api-keys' },
        ...(canManageUsers
          ? [
              {
                key: 'user-management',
                title: 'User Management',
                to: '/ui/management/',
                external: true,
              },
            ]
          : []),
        ...edition.settings,
      ],
    }),
    [canManageUsers]
  );

  const isHomePage = location.pathname === '/';

  return (
    <SidebarMenu>
      <SidebarGroup className="gap-0.5 group-data-[state=collapsed]:items-center">
        {allowedMenu.map((menuItem) => (
          <SideBarNavLink
            key={menuItem.key}
            route={menuItem}
            tooltip={menuItem.title}
            active={
              location.pathname.startsWith(menuItem.to) ||
              (menuItem.to === '/workflows' && isHomePage)
            }
          />
        ))}
        <SideBarNavLink
          key={settingsItem.key}
          route={settingsItem}
          tooltip={settingsItem.title}
          active={location.pathname.startsWith('/settings')}
        />
      </SidebarGroup>
    </SidebarMenu>
  );
}

function FooterMenu() {
  // Settings (with API Keys / User Management) is a group in the main nav above,
  // so there's no footer gear here.
  const versionLabel = formatBuildLabel(
    config.build.version,
    config.build.commit,
    config.build.buildNumber
  );
  const versionTitle = formatBuildTitle(
    config.build.version,
    config.build.commit,
    config.build.buildNumber
  );
  // Utilities, not actions: neutral like the nav, not primary blue.
  const iconButton = 'size-8 text-muted-foreground hover:text-foreground';

  return (
    <div className="flex min-w-0 items-center justify-between gap-2 border-t border-sidebar-border px-1.5 pt-3 group-data-[state=collapsed]:flex-col group-data-[state=collapsed]:border-t-0 group-data-[state=collapsed]:px-0 group-data-[state=collapsed]:pt-0">
      <span
        className="min-w-0 truncate pl-1 font-mono text-2xs text-muted-foreground group-data-[state=collapsed]:hidden"
        title={versionTitle}
        aria-label={`Runtara version ${versionLabel}`}
      >
        {versionLabel}
      </span>
      <div className="flex shrink-0 items-center gap-0.5 group-data-[state=collapsed]:flex-col group-data-[state=collapsed]:gap-1">
        <TenantSwitcher className={iconButton} />
        <AuthSidebar className={iconButton} />
        {edition.footer.map((Control, index) => (
          <Control key={index} className={iconButton} />
        ))}
        <ThemeSwitcher className={iconButton} />
        <SidebarTrigger className={cn('mx-0 shrink-0 px-0', iconButton)} />
      </div>
    </div>
  );
}

function formatBuildLabel(
  version: string,
  commit?: string,
  buildNumber?: string
): string {
  const normalizedVersion = normalizeVersion(version);
  const normalizedCommit = normalizeCommit(commit);
  const build = buildNumber?.trim() || undefined;

  let label =
    normalizedVersion === 'dev' && normalizedCommit
      ? `dev@${normalizedCommit}`
      : normalizedVersion;

  if (build) {
    label += ` (${build})`;
  }

  return label;
}

function formatBuildTitle(
  version: string,
  commit?: string,
  buildNumber?: string
): string {
  const normalizedVersion = normalizeVersion(version);
  const normalizedCommit = normalizeCommit(commit);
  const build = buildNumber?.trim() || undefined;

  const parts: string[] = [];
  if (normalizedCommit) parts.push(normalizedCommit);
  if (build) parts.push(`build #${build}`);

  if (parts.length > 0) {
    return `Runtara ${normalizedVersion} (${parts.join(', ')})`;
  }

  return `Runtara ${normalizedVersion}`;
}

function normalizeVersion(version: string): string {
  const trimmed = version.trim();
  if (!trimmed) return 'dev';
  if (trimmed === 'dev' || trimmed.startsWith('v')) return trimmed;
  return `v${trimmed}`;
}

function normalizeCommit(commit?: string): string | undefined {
  const trimmed = commit?.trim();
  if (!trimmed || trimmed === 'unknown') return undefined;
  return trimmed.slice(0, 12);
}

function SideBarNavLink({
  route,
  active,
  tooltip,
}: {
  route: any;
  active?: boolean;
  tooltip?: string;
} & ComponentPropsWithoutRef<typeof SidebarMenuButton>) {
  const location = useLocation();
  const [searchParams] = useSearchParams();
  const { isMobile, setOpenMobile } = useSidebar();

  const handleNavClick = () => {
    if (isMobile) setOpenMobile(false);
  };

  const currentUrl = `${location.pathname}${searchParams.toString() ? `?${searchParams.toString()}` : ''}`;
  const isChildActive = (child: any) =>
    !child.external &&
    (child.to === currentUrl || location.pathname === child.to);
  // With a sub-item highlighted, the parent only names the section: one
  // filled item in the nav, not two stacked on each other.
  const childActive = !!route.children?.some(isChildActive);

  const buttonProps = {
    isActive: active && !childActive,
    className: childActive
      ? 'font-semibold text-foreground [&>svg]:text-primary-text'
      : active
        ? ''
        : 'text-muted-foreground',
    size: 'default' as const,
    tooltip,
    disabled: route.disabled,
  };

  const content = (
    <>
      {route.icon}
      <span>{route.title}</span>
      {route.disabled && (
        <span className="ml-auto text-xs text-muted-foreground">
          Coming soon
        </span>
      )}
    </>
  );

  return (
    <SidebarMenuItem>
      {route.disabled ? (
        <SidebarMenuButton {...buttonProps}>{content}</SidebarMenuButton>
      ) : (
        <SidebarMenuButton {...buttonProps} asChild onClick={handleNavClick}>
          <Link to={route.to}>{content}</Link>
        </SidebarMenuButton>
      )}
      {route.children && route.children.length > 0 && (
        <SidebarMenuSub>
          {route.children.map((child: any) => (
            <SidebarMenuSubItem key={child.key}>
              <SidebarMenuSubButton
                asChild
                size="md"
                isActive={isChildActive(child)}
                className={cn(
                  'h-8 text-[13px]',
                  !isChildActive(child) && 'text-muted-foreground'
                )}
                onClick={handleNavClick}
              >
                {child.external ? (
                  // Leaves the SPA (e.g. the management SPA): a plain anchor so
                  // the browser does a full navigation, not a react-router push.
                  <a href={child.to}>
                    {child.icon}
                    {child.title}
                  </a>
                ) : (
                  <Link to={child.to}>
                    {child.icon}
                    {child.title}
                  </Link>
                )}
              </SidebarMenuSubButton>
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
}
