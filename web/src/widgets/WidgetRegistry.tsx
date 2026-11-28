import { lazy, Suspense, type ComponentType } from "react";
import { ActionChips } from "./ActionChips";
import { ApprovalDiff } from "./ApprovalDiff";
import { DataTable } from "./DataTable";
import { InteractiveForm } from "./InteractiveForm";
import type { WidgetProps } from "./protocol";
import { WidgetCard } from "./WidgetCard";

// Charts pull in recharts: loaded on first use.
const MetricsChart = lazy(() => import("./MetricsChart"));

function LazyChart(props: WidgetProps) {
  return (
    <Suspense fallback={<WidgetCard title={props.spec.title} kind="metrics"><div className="muted">グラフを読み込み中…</div></WidgetCard>}>
      <MetricsChart {...props} />
    </Suspense>
  );
}

/** widget_type → component. Add new widgets here. */
export const WIDGETS: Record<string, ComponentType<WidgetProps>> = {
  metrics_chart: LazyChart,
  data_table: DataTable,
  interactive_form: InteractiveForm,
  approval_diff: ApprovalDiff,
  action_chips: ActionChips,
};

/** Unknown widgets never break the chat: they show as collapsible JSON. */
function Fallback({ spec }: WidgetProps) {
  return (
    <details className="widget widget-unknown">
      <summary>
        表示できないカード <code>{spec.widget_type}</code>
      </summary>
      <pre className="json">{JSON.stringify(spec, null, 2)}</pre>
    </details>
  );
}

export function Widget(props: WidgetProps) {
  const C = WIDGETS[props.spec.widget_type] ?? Fallback;
  return <C {...props} />;
}
