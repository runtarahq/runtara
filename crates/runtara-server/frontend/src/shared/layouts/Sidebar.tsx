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
import { menu } from '@/shared/config';
import { config } from '@/shared/config/runtimeConfig';
import Logo from '@/assets/logo/runtara-logo-icon.svg';
import { AuthSidebar } from './AuthSidebar.tsx';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { checkUserGroup } from '@/lib/utils.ts';
import { ThemeSwitcher } from '@/shared/components/theme-switcher.tsx';
import { DollarSign, Folder, Settings } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { useCustomMutation } from '@/shared/hooks/api';
import { createBillingPortalSession } from '@/shared/queries';
import { toast } from 'sonner';
import { useNavigate } from 'react-router';
import { useFolders } from '@/features/workflows/hooks/useFolders';

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
        <div className="flex aspect-square size-8 items-center justify-center rounded-sm text-sidebar-primary-foreground">
          <img src={Logo} alt="Runtara logo" />
        </div>
        <div className="grid flex-1 text-left leading-tight group-data-[collapsible=icon]:hidden">
          <span className="truncate text-lg font-semibold text-slate-900/90 dark:text-slate-100">
            Runtara
          </span>
          <span
            role="link"
            tabIndex={0}
            className="text-[11px] text-gray-500 no-underline hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-300 cursor-pointer"
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
  const { data: foldersData } = useFolders();

  const userGroups = useAuthStore((state) => state.userGroups);

  const allowedMenu = useMemo(() => {
    return menu.filter((menuItem) => {
      const { allowedGroups } = menuItem;
      return checkUserGroup(allowedGroups, userGroups);
    });
  }, [userGroups]);

  const menuWithFolders = useMemo(() => {
    return allowedMenu.map((menuItem) => {
      if (
        menuItem.key === 'workflows' &&
        foldersData?.root &&
        foldersData.root.length > 0
      ) {
        return {
          ...menuItem,
          children: foldersData.root.map((folder) => ({
            key: folder.path,
            title: folder.name,
            to: `/workflows?folder=${encodeURIComponent(folder.path)}`,
            icon: <Folder className="h-4 w-4 text-amber-500" />,
          })),
        };
      }
      return menuItem;
    });
  }, [allowedMenu, foldersData?.root]);

  const isHomePage = location.pathname === '/';

  return (
    <SidebarMenu>
      <SidebarGroup className="gap-0.5 group-data-[state=collapsed]:items-center">
        {menuWithFolders.map((menuItem) => (
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
      </SidebarGroup>
    </SidebarMenu>
  );
}

function FooterMenu() {
  const navigate = useNavigate();
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
  const { mutate: createPortalSession, isPending } = useCustomMutation({
    mutationFn: createBillingPortalSession,
    onSuccess: (data: { url: string }) => {
      window.location.href = data.url;
    },
    onError: () => {
      toast.error('Failed to create billing portal session');
    },
  });

  return (
    <div className="flex min-w-0 flex-col gap-1 px-2 py-2">
      <div className="flex items-center justify-center gap-2 group-data-[state=collapsed]:flex-col group-data-[state=collapsed]:gap-1">
        <AuthSidebar />
        <Button
          variant="ghost"
          size="icon"
          className="relative h-9 w-9 shrink-0"
          aria-label="Settings"
          onClick={() => navigate('/settings/api-keys')}
        >
          <Settings className="h-4 w-4" />
          <span className="sr-only">Settings</span>
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="relative h-9 w-9 shrink-0"
          aria-label="Manage billing"
          onClick={() => createPortalSession({})}
          disabled={isPending}
        >
          <DollarSign className="h-4 w-4" />
          <span className="sr-only">Manage billing</span>
        </Button>
        <ThemeSwitcher />
        <SidebarTrigger className="w-min px-0 mx-0 shrink-0" />
      </div>
      <div
        className="min-w-0 px-1 text-center text-[11px] font-medium text-muted-foreground/80 group-data-[state=collapsed]:hidden"
        title={versionTitle}
        aria-label={`Runtara version ${versionLabel}`}
      >
        <span className="block truncate">{versionLabel}</span>
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

  const buttonProps = {
    isActive: active,
    className: active ? '' : 'text-muted-foreground',
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
                size="sm"
                isActive={
                  child.to === currentUrl || location.pathname === child.to
                }
                onClick={handleNavClick}
              >
                <Link to={child.to}>
                  {child.icon}
                  {child.title}
                </Link>
              </SidebarMenuSubButton>
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
}
