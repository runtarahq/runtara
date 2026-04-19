import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';

type Props = {
  className?: string;
  type: string;
};

export function StepTypeIcon(props: Props) {
  const { className, type } = props;

  const iconClassName = cn('w-4 h-4', className);

  switch (type) {
    case 'Create':
      return <Icons.add className={iconClassName} />;
    case 'Agent':
      return <Icons.agent className={iconClassName} />;
    case 'EmbedWorkflow':
    case 'Start Workflow': // Backend format (with space)
      return <Icons.rocket className={iconClassName} />;
    case 'Terminate':
      return <Icons.stop className={iconClassName} />;
    case 'Conditional':
      return <Icons.split className={iconClassName} />;
    case 'Split':
      return <Icons.asterisk className={iconClassName} />;
    case 'Switch':
      return <Icons.squareMenu className={iconClassName} />;
    case 'Combine':
      return <Icons.merge className={iconClassName} />;
    case 'Wait':
      return <Icons.wait className={iconClassName} />;
    case 'Event':
      return <Icons.event className={iconClassName} />;
    case 'GroupBy':
      return <Icons.groupby className={iconClassName} />;
    case 'Start': // Backend format (with space)
      return <Icons.entryPoint className={iconClassName} />;
    case 'Finish':
      return <Icons.finish className={iconClassName} />;
    case 'Error':
      return <Icons.warning className={iconClassName} />;
    case 'Filter':
      return <Icons.filter className={iconClassName} />;
    case 'AiAgent':
    case 'AI Agent':
      return <Icons.sparkles className={iconClassName} />;
    case 'WaitForSignal':
    case 'Wait For Signal':
      return <Icons.wait className={iconClassName} />;
    default:
      return <Icons.gear className={iconClassName} />;
  }
}
