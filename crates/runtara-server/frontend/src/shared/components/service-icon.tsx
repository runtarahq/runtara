import {
  Server,
  Database,
  Mail,
  Cloud,
  Globe,
  FileText,
  ShoppingCart,
  CreditCard,
  MessageSquare,
  Link2,
  LucideIcon,
} from 'lucide-react';

type ServiceIconProps = {
  serviceId?: string;
  category?: string;
  className?: string;
};

const SERVICE_ICONS: Record<string, { icon: LucideIcon; gradient: string }> = {
  sftp: { icon: Server, gradient: 'from-emerald-500 to-teal-600' },
  ftp: { icon: Server, gradient: 'from-emerald-500 to-teal-600' },
  mysql: { icon: Database, gradient: 'from-blue-500 to-indigo-600' },
  postgres: { icon: Database, gradient: 'from-blue-500 to-indigo-600' },
  postgresql: { icon: Database, gradient: 'from-blue-500 to-indigo-600' },
  mongodb: { icon: Database, gradient: 'from-green-500 to-emerald-600' },
  smtp: { icon: Mail, gradient: 'from-amber-500 to-orange-600' },
  email: { icon: Mail, gradient: 'from-amber-500 to-orange-600' },
  aws: { icon: Cloud, gradient: 'from-orange-500 to-amber-600' },
  azure: { icon: Cloud, gradient: 'from-blue-500 to-cyan-600' },
  gcp: { icon: Cloud, gradient: 'from-blue-500 to-green-600' },
  http: { icon: Globe, gradient: 'from-violet-500 to-purple-600' },
  rest: { icon: Globe, gradient: 'from-violet-500 to-purple-600' },
  api: { icon: Globe, gradient: 'from-violet-500 to-purple-600' },
  csv: { icon: FileText, gradient: 'from-slate-500 to-gray-600' },
  shopify: { icon: ShoppingCart, gradient: 'from-green-500 to-lime-600' },
  stripe: { icon: CreditCard, gradient: 'from-violet-500 to-indigo-600' },
  slack: { icon: MessageSquare, gradient: 'from-pink-500 to-rose-600' },
  webhook: { icon: Link2, gradient: 'from-cyan-500 to-blue-600' },
};

const CATEGORY_ICONS: Record<string, { icon: LucideIcon; gradient: string }> = {
  file_storage: { icon: Server, gradient: 'from-emerald-500 to-teal-600' },
  database: { icon: Database, gradient: 'from-blue-500 to-indigo-600' },
  email: { icon: Mail, gradient: 'from-amber-500 to-orange-600' },
  cloud: { icon: Cloud, gradient: 'from-sky-500 to-blue-600' },
  api: { icon: Globe, gradient: 'from-violet-500 to-purple-600' },
  ecommerce: { icon: ShoppingCart, gradient: 'from-green-500 to-lime-600' },
  payment: { icon: CreditCard, gradient: 'from-violet-500 to-indigo-600' },
  messaging: { icon: MessageSquare, gradient: 'from-pink-500 to-rose-600' },
};

export function ServiceIcon({
  serviceId,
  category,
  className = 'w-10 h-10',
}: ServiceIconProps) {
  // Try to match by service ID first
  const serviceKey = serviceId?.toLowerCase();
  let iconConfig = serviceKey ? SERVICE_ICONS[serviceKey] : undefined;

  // Fall back to category matching
  if (!iconConfig && category) {
    const categoryKey = category.toLowerCase().replace(/\s+/g, '_');
    iconConfig = CATEGORY_ICONS[categoryKey];
  }

  // Default fallback
  if (!iconConfig) {
    iconConfig = { icon: Link2, gradient: 'from-slate-500 to-gray-600' };
  }

  const { icon: Icon, gradient } = iconConfig;

  return (
    <div
      className={`${className} bg-gradient-to-br ${gradient} rounded-xl flex items-center justify-center shadow-lg`}
      style={{
        boxShadow: `0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -4px rgba(0, 0, 0, 0.1)`,
      }}
    >
      <Icon className="w-1/2 h-1/2 text-white" />
    </div>
  );
}
