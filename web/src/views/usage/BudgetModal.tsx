import { useEffect, useState, type FormEvent } from "react";
import { errorText } from "../../api";
import { analytics, BUDGET_CHANGED, type PricingOverride } from "../../api/analytics";
import { ErrorBar, Modal } from "../../components/common";
import { Checkbox, SegmentedControl } from "../../components/ui";
import { useT, type Key } from "../../i18n";

export type BudgetTab = "budget" | "pricing";

const PRICES = [
  { key: "input_per_m", label: "usage.price.input", required: true },
  { key: "cached_input_per_m", label: "usage.price.cached", required: false },
  { key: "cache_write_per_m", label: "usage.price.cacheWrite", required: false },
  { key: "output_per_m", label: "usage.price.output", required: true },
] as const satisfies { key: keyof PricingOverride; label: Key; required: boolean }[];

type PriceKey = (typeof PRICES)[number]["key"];
type Draft = { model_pattern: string } & Record<PriceKey, string>;

const emptyDraft = (model = ""): Draft => ({ model_pattern: model, input_per_m: "", cached_input_per_m: "", cache_write_per_m: "", output_per_m: "" });

/** `null` for an empty field, `undefined` for something that is not a non-negative number. */
function amount(text: string): number | null | undefined {
  const s = text.trim();
  if (!s) return null;
  const n = Number(s);
  return Number.isFinite(n) && n >= 0 ? n : undefined;
}

function BudgetForm({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const [limit, setLimit] = useState("");
  const [threshold, setThreshold] = useState("80");
  const [scoped, setScoped] = useState<unknown>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    analytics.budget().then(
      (b) => {
        setLimit(b.monthly_budget_usd?.toString() ?? "");
        setThreshold(String(b.warning_threshold_pct));
        setScoped(b.scoped_budgets);
        setLoaded(true);
      },
      (e) => setError(errorText(e)),
    );
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const monthly = amount(limit);
    const pct = Number(threshold);
    if (monthly === undefined || monthly === 0) return setError(t("usage.budget.invalidLimit"));
    if (!Number.isInteger(pct) || pct < 1 || pct > 100) return setError(t("usage.budget.invalidThreshold"));
    try {
      await analytics.setBudget(monthly, pct, scoped);
      setError(null);
      setSaved(true);
      onSaved();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <form className="u-form" onSubmit={submit}>
      <ErrorBar error={error} />
      <label className="u-field">
        <span>{t("usage.budget.limit")}</span>
        <input inputMode="decimal" value={limit} placeholder={t("usage.budget.noLimit")} onChange={(e) => (setLimit(e.target.value), setSaved(false))} />
      </label>
      <label className="u-field">
        <span>{t("usage.budget.threshold")}</span>
        <input inputMode="numeric" value={threshold} onChange={(e) => (setThreshold(e.target.value), setSaved(false))} />
      </label>
      <p className="u-hint">{t("usage.budget.hint")}</p>
      <div className="u-form-actions">
        {saved && <span className="u-hint">{t("usage.saved.ok")}</span>}
        <button type="submit" className="btn primary" disabled={!loaded}>
          {t("common.save")}
        </button>
      </div>
    </form>
  );
}

function PricingForm({ initialModel, unpriced, onSaved }: { initialModel: string; unpriced: string[]; onSaved: () => void }) {
  const t = useT();
  const [list, setList] = useState<PricingOverride[]>([]);
  const [draft, setDraft] = useState<Draft>(() => emptyDraft(initialModel));
  const [recalculate, setRecalculate] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    analytics.pricing().then(setList, (e) => setError(errorText(e)));
  }, []);

  const run = async (op: () => Promise<PricingOverride[]>) => {
    try {
      setList(await op());
      setError(null);
      onSaved();
      return true;
    } catch (e) {
      setError(errorText(e));
      return false;
    }
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const pattern = draft.model_pattern.trim();
    const values = PRICES.map((p) => ({ ...p, value: amount(draft[p.key]) }));
    if (!pattern || values.some((v) => v.value === undefined || (v.required && v.value === null))) {
      return setError(t("usage.price.invalid"));
    }
    const pricing = Object.fromEntries(values.map((v) => [v.key, v.value])) as Record<PriceKey, number | null>;
    if (await run(() => analytics.setPricing({ model_pattern: pattern, ...pricing }, recalculate))) setDraft(emptyDraft());
  };

  const edit = (p: PricingOverride) =>
    setDraft({
      model_pattern: p.model_pattern,
      input_per_m: p.input_per_m?.toString() ?? "",
      cached_input_per_m: p.cached_input_per_m?.toString() ?? "",
      cache_write_per_m: p.cache_write_per_m?.toString() ?? "",
      output_per_m: p.output_per_m?.toString() ?? "",
    });

  const missing = unpriced.filter((m) => !list.some((p) => p.model_pattern === m));

  return (
    <div className="u-form">
      <ErrorBar error={error} />
      <p className="u-hint">{t("usage.price.hint")}</p>
      {list.length === 0 ? (
        <p className="u-hint">{t("usage.price.none")}</p>
      ) : (
        <ul className="u-prices">
          {list.map((p) => (
            <li key={p.model_pattern}>
              <button type="button" className="text-btn mono u-price-name" onClick={() => edit(p)}>
                {p.model_pattern}
              </button>
              <span className="u-num u-hint">{PRICES.map((f) => p[f.key] ?? "–").join(" / ")}</span>
              <button type="button" className="text-btn" onClick={() => run(() => analytics.deletePricing(p.model_pattern, recalculate))}>
                {t("common.delete")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {missing.length > 0 && (
        <div className="u-missing">
          <span className="u-hint">{t("usage.price.unpriced")}</span>
          {missing.map((m) => (
            <button key={m} type="button" className="btn small mono" onClick={() => setDraft(emptyDraft(m))}>
              {m}
            </button>
          ))}
        </div>
      )}
      <form onSubmit={submit} className="u-form">
        <label className="u-field">
          <span>{t("usage.price.pattern")}</span>
          <input className="mono" value={draft.model_pattern} placeholder="gpt-5*" onChange={(e) => setDraft({ ...draft, model_pattern: e.target.value })} />
        </label>
        <div className="u-price-grid">
          {PRICES.map((p) => (
            <label key={p.key} className="u-field">
              <span>{t(p.label)}</span>
              <input
                inputMode="decimal"
                value={draft[p.key]}
                placeholder={p.required ? "0.00" : t("usage.price.sameAsInput")}
                onChange={(e) => setDraft({ ...draft, [p.key]: e.target.value })}
              />
            </label>
          ))}
        </div>
        <Checkbox checked={recalculate} onChange={setRecalculate} label={t("usage.price.recalculate")} />
        <div className="u-form-actions">
          <button type="submit" className="btn primary">
            {t("usage.price.save")}
          </button>
        </div>
      </form>
    </div>
  );
}

/** Monthly budget and custom unit prices. */
export function BudgetModal({
  initialTab,
  initialModel,
  unpriced,
  onClose,
  onChanged,
}: {
  initialTab: BudgetTab;
  initialModel: string;
  unpriced: string[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const t = useT();
  const [tab, setTab] = useState(initialTab);
  const saved = () => {
    window.dispatchEvent(new Event(BUDGET_CHANGED));
    onChanged();
  };
  return (
    <Modal title={t("usage.configure")} onClose={onClose}>
      <SegmentedControl<BudgetTab>
        full
        ariaLabel={t("usage.configure")}
        value={tab}
        onChange={setTab}
        options={[
          { value: "budget", label: t("usage.tab.budget") },
          { value: "pricing", label: t("usage.tab.pricing") },
        ]}
      />
      {tab === "budget" ? <BudgetForm onSaved={saved} /> : <PricingForm initialModel={initialModel} unpriced={unpriced} onSaved={saved} />}
    </Modal>
  );
}
