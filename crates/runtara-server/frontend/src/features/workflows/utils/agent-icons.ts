import {
  Wrench,
  ArrowRightLeft,
  Lock,
  Table,
  Type,
  FileCode,
  Clock,
  Globe,
  HardDriveUpload,
  Archive,
  File,
  Database,
  Sparkles,
  Cloud,
  ShoppingCart,
  FlaskConical,
  Brain,
  ShoppingBag,
  Bot,
  type LucideIcon,
} from 'lucide-react';

const agentIconMap: Record<string, LucideIcon> = {
  utils: Wrench,
  transform: ArrowRightLeft,
  crypto: Lock,
  csv: Table,
  text: Type,
  xml: FileCode,
  datetime: Clock,
  http: Globe,
  sftp: HardDriveUpload,
  compression: Archive,
  file: File,
  object_model: Database,
  openai: Sparkles,
  bedrock: Cloud,
  hdm_commerce: ShoppingCart,
  'smo-test': FlaskConical,
  hdm_llm: Brain,
  shopify: ShoppingBag,
};

export function getAgentIcon(agentId: string | null | undefined): LucideIcon {
  if (!agentId) return Bot;
  return agentIconMap[agentId.toLowerCase()] || Bot;
}
