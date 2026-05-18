import { Label } from '@/shared/components/ui/label';
import { Textarea } from '@/shared/components/ui/textarea';
import { ReportBlockDefinition } from '../../../types';

interface MarkdownBlockEditorProps {
  block: ReportBlockDefinition;
  onChange: (block: ReportBlockDefinition) => void;
}

export function MarkdownBlockEditor({
  block,
  onChange,
}: MarkdownBlockEditorProps) {
  const content = block.markdown?.content ?? '';
  return (
    <div className="grid gap-1.5">
      <Label className="text-xs" htmlFor={`md_${block.id}`}>
        Markdown content
      </Label>
      <Textarea
        id={`md_${block.id}`}
        value={content}
        rows={8}
        onChange={(event) =>
          onChange({
            ...block,
            markdown: { ...(block.markdown ?? {}), content: event.target.value },
          })
        }
        placeholder="Markdown supports headings, lists, links…"
      />
    </div>
  );
}
