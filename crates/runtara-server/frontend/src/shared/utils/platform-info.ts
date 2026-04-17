/**
 * Platform Information Utility
 *
 * Provides metadata and helper functions for platform integrations
 * Used in HDM (High-Level Data Model) UI components to display
 * platform-specific information like icons, names, and colors.
 */

interface PlatformMetadata {
  name: string;
  icon: string;
  color?: string;
  description?: string;
}

/**
 * Platform metadata mapped by integration ID
 * Integration IDs come from the backend and identify specific platform implementations
 */
const PLATFORM_INFO: Record<string, PlatformMetadata> = {
  // E-Commerce Platforms
  shopify_commerce: {
    name: 'Shopify',
    icon: '🛍️',
    color: '#95BF47',
    description: 'Shopify e-commerce platform',
  },
  woocommerce_commerce: {
    name: 'WooCommerce',
    icon: '🛒',
    color: '#96588A',
    description: 'WooCommerce for WordPress',
  },
  bigcommerce_commerce: {
    name: 'BigCommerce',
    icon: '🏪',
    color: '#1D4E89',
    description: 'BigCommerce platform',
  },

  // CRM Platforms (examples - add as they become available)
  salesforce_crm: {
    name: 'Salesforce',
    icon: '☁️',
    color: '#00A1E0',
    description: 'Salesforce CRM',
  },
  hubspot_crm: {
    name: 'HubSpot',
    icon: '🎯',
    color: '#FF7A59',
    description: 'HubSpot CRM',
  },

  // Generic/Default
  http: {
    name: 'HTTP',
    icon: '🌐',
    color: '#6B7280',
    description: 'Generic HTTP integration',
  },
};

/**
 * Default platform metadata for unknown integration IDs
 */
const DEFAULT_PLATFORM: PlatformMetadata = {
  name: 'Unknown',
  icon: '🔌',
  color: '#9CA3AF',
  description: 'Unknown platform',
};

/**
 * Get platform metadata by integration ID
 * @param integrationId - The integration ID from connection or operator
 * @returns Platform metadata object
 */
export function getPlatformInfo(
  integrationId?: string | null
): PlatformMetadata {
  if (!integrationId) {
    return DEFAULT_PLATFORM;
  }

  return PLATFORM_INFO[integrationId] || DEFAULT_PLATFORM;
}

/**
 * Get platform display name
 * @param integrationId - The integration ID
 * @returns Platform name string
 */
export function getPlatformName(integrationId?: string | null): string {
  return getPlatformInfo(integrationId).name;
}

/**
 * Get platform icon emoji
 * @param integrationId - The integration ID
 * @returns Platform icon emoji string
 */
export function getPlatformIcon(integrationId?: string | null): string {
  return getPlatformInfo(integrationId).icon;
}

/**
 * Get platform color for UI theming
 * @param integrationId - The integration ID
 * @returns Hex color string
 * @lintignore Public helper exported alongside getPlatformName/Icon for consumer UI use.
 */
export function getPlatformColor(integrationId?: string | null): string {
  return getPlatformInfo(integrationId).color || DEFAULT_PLATFORM.color!;
}

/**
 * Check if an integration ID is a known platform
 * @param integrationId - The integration ID
 * @returns true if platform is known
 * @lintignore Public helper exported alongside getPlatformName/Icon for consumer UI use.
 */
export function isKnownPlatform(integrationId?: string | null): boolean {
  if (!integrationId) return false;
  return integrationId in PLATFORM_INFO;
}

/**
 * Get all available platforms
 * @returns Array of [integrationId, metadata] tuples
 * @lintignore Public helper exported alongside getPlatformName/Icon for consumer UI use.
 */
export function getAllPlatforms(): [string, PlatformMetadata][] {
  return Object.entries(PLATFORM_INFO);
}

/**
 * Group integration IDs by platform type (e.g., commerce, crm)
 * @param integrationIds - Array of integration IDs
 * @returns Grouped platform information
 * @lintignore Public helper exported alongside getPlatformName/Icon for consumer UI use.
 */
export function groupPlatformsByType(
  integrationIds: string[]
): Record<string, PlatformMetadata[]> {
  const grouped: Record<string, PlatformMetadata[]> = {};

  for (const id of integrationIds) {
    const info = getPlatformInfo(id);

    // Extract type from integration ID (e.g., "shopify_commerce" -> "commerce")
    const typeSuffix = id.split('_').pop() || 'other';
    const type = typeSuffix.charAt(0).toUpperCase() + typeSuffix.slice(1);

    if (!grouped[type]) {
      grouped[type] = [];
    }

    grouped[type].push({ ...info, ...{ integrationId: id } } as any);
  }

  return grouped;
}
