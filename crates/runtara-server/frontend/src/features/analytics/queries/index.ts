import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

// Re-export relevant DTOs from the generated Runtime API for easier access
export type {
  TenantMetricsResponse,
  TenantMetricsData,
  TenantMetricsDataPoint,
  ScenarioMetricsHourlyResponse,
  ScenarioMetricsHourlyData,
  ScenarioMetricsHourly,
  ScenarioMetricsDaily,
  ScenarioMetricsDailyResponse,
  ScenarioMetricsData,
  ScenarioStatsResponse,
  ScenarioStatsData,
  ScenarioStats,
  SystemAnalyticsResponse,
  SystemAnalyticsData,
  CpuInfo,
  DiskInfo,
  MemoryInfo,
} from '@/generated/RuntaraRuntimeApi';

/**
 * Get tenant-level metrics aggregated across all scenarios
 * @param token - Authentication token
 * @param startTime - Start time in ISO 8601 format
 * @param endTime - End time in ISO 8601 format
 */
export async function getTenantMetrics(
  token: string,
  startTime: string,
  endTime: string
) {
  const result = await RuntimeREST.api.getTenantMetrics(
    {
      startTime,
      endTime,
    },
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Get system analytics including memory, disk space, and CPU information
 * @param token - Authentication token
 */
export async function getSystemAnalytics(token: string) {
  const result = await RuntimeREST.api.getSystemAnalyticsHandler(
    createAuthHeaders(token)
  );
  return result.data;
}
