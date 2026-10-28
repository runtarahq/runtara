import type { ComponentType, ReactNode } from 'react';
import type { MenuChild, MenuItem } from '@/shared/config';

/** A page an edition adds, mounted in the app layout behind sign-in. */
export type EditionRoute = {
  path: string;
  element: ReactNode;
};

/**
 * What a build's edition adds to the console. The open-source build's edition
 * (`src/edition`) adds nothing; a distribution such as Runtara Cloud builds the
 * console with the `@edition` alias pointed at its own module
 * (`RUNTARA_EDITION_DIR`) to add its pages, menu entries and controls without
 * carrying them in this repository.
 */
export type Edition = {
  /** Pages, after the built-in ones. */
  routes: EditionRoute[];
  /**
   * Pages for any signed-in user, whether or not they belong to the tenant
   * (accepting an invitation to it, say): shown outside the app layout and
   * before the tenant gates.
   */
  open: EditionRoute[];
  /** Main-menu entries, after the built-in ones. */
  menu: MenuItem[];
  /** Entries in the Settings group, after API Keys. */
  settings: MenuChild[];
  /** Sidebar-footer controls, before the theme switcher. */
  footer: ComponentType<{ className?: string }>[];
};
