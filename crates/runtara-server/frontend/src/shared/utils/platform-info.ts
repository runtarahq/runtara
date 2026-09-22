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
