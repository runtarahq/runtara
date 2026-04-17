import { ManagementAPIClient } from '@/shared/queries';
import { useMaintenanceStore } from '@/shared/stores/maintenanceStore';
import { useState } from 'react';
import logoIcon from '@/assets/logo/runtara-logo-icon.svg';

export function MaintenancePage() {
  const setMaintenanceMode = useMaintenanceStore((s) => s.setMaintenanceMode);
  const [checking, setChecking] = useState(false);

  const handleRetry = async () => {
    setChecking(true);
    try {
      await ManagementAPIClient.instance.get('/health');
      setMaintenanceMode(false);
    } catch {
      // still in maintenance
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className="relative flex min-h-screen items-center justify-center overflow-hidden bg-white p-4">
      {/* Ambient glow */}
      <div className="pointer-events-none absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2">
        <div className="h-[600px] w-[600px] rounded-full bg-blue-500/[0.06] blur-[120px]" />
      </div>

      <div className="relative z-10 w-full max-w-lg text-center">
        {/* Logo */}
        <div className="mb-10 flex justify-center">
          <img src={logoIcon} alt="Runtara" className="h-12 w-12" />
        </div>

        {/* Card */}
        <div className="relative rounded-xl border border-gray-200 bg-white shadow-lg shadow-gray-200/50">
          {/* Gradient top accent line */}
          <div className="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-blue-500 to-transparent" />

          <div className="px-8 pt-10 pb-8">
            {/* Animated pulse indicator */}
            <div className="mx-auto mb-8 flex h-14 w-14 items-center justify-center">
              <span className="absolute inline-flex h-10 w-10 animate-ping rounded-full bg-blue-500/10" />
              <span className="relative inline-flex h-10 w-10 items-center justify-center rounded-full border border-blue-200 bg-blue-50">
                <svg
                  className="h-5 w-5 text-blue-500"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth={1.5}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
                </svg>
              </span>
            </div>

            <h1 className="mb-3 text-2xl font-semibold tracking-tight text-gray-900">
              Scheduled Maintenance
            </h1>

            <p className="mx-auto max-w-sm leading-relaxed text-gray-500">
              We're performing planned improvements to bring you a better
              experience. The system will be back online shortly.
            </p>

            {/* Status row */}
            <div className="mt-8 flex items-center justify-center gap-2 text-sm text-gray-400">
              <span className="relative flex h-2 w-2">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-amber-400 opacity-75" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-amber-400" />
              </span>
              Maintenance in progress
            </div>
          </div>

          {/* Footer / retry */}
          <div className="border-t border-gray-100 px-8 py-5">
            <button
              onClick={handleRetry}
              disabled={checking}
              className="inline-flex items-center gap-2 rounded-lg bg-gray-900 px-5 py-2.5 text-sm font-medium text-white shadow-sm transition-all hover:-translate-y-0.5 hover:shadow-md disabled:opacity-50 disabled:hover:translate-y-0"
            >
              {checking ? (
                <>
                  <svg
                    className="h-4 w-4 animate-spin"
                    viewBox="0 0 24 24"
                    fill="none"
                  >
                    <circle
                      className="opacity-25"
                      cx="12"
                      cy="12"
                      r="10"
                      stroke="currentColor"
                      strokeWidth="4"
                    />
                    <path
                      className="opacity-75"
                      fill="currentColor"
                      d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
                    />
                  </svg>
                  Checking...
                </>
              ) : (
                'Check Again'
              )}
            </button>
          </div>
        </div>

        {/* Footer text */}
        <p className="mt-8 text-xs text-gray-400">
          This page refreshes automatically every 30 seconds.
        </p>
      </div>
    </div>
  );
}
