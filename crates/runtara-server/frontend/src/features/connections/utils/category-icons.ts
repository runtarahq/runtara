import {
  Plug,
  ShoppingCart,
  FolderArchive,
  Brain,
  Users,
  Building2,
  Database,
  Mail,
  MessageSquare,
  CreditCard,
  Cloud,
  Cable,
  type LucideIcon,
} from 'lucide-react';

const categoryIconMap: Record<string, LucideIcon> = {
  ecommerce: ShoppingCart,
  file_storage: FolderArchive,
  llm: Brain,
  crm: Users,
  erp: Building2,
  database: Database,
  email: Mail,
  messaging: MessageSquare,
  payment: CreditCard,
  cloud: Cloud,
  api: Cable,
};

export function getCategoryIcon(
  category: string | null | undefined
): LucideIcon {
  if (!category) return Plug;
  return categoryIconMap[category.toLowerCase()] || Plug;
}

export function getCategoryLabel(category: string | null | undefined): string {
  if (!category) return 'General';
  return category
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
}
