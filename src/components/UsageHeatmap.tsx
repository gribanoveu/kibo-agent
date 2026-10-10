import { useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { useUsage } from "../hooks/useUsage";
import {
  HEATMAP_WEEKS,
  shortDay,
  usageHeatmap,
  usageStats,
  usageStrip,
  type Ranked,
  type UsageDay,
  type UsageRange,
} from "../lib/usage";
import { formatTokens } from "./ChatPanel";
import "./UsageHeatmap.css";

const RANGES: { value: UsageRange; label: string }[] = [
  { value: "all", label: "All time" },
  { value: "30", label: "30 days" },
  { value: "7", label: "7 days" },
];

const WEEKDAYS = ["Mon", "", "Wed", "", "Fri", "", ""];

/** How many days the new chat's strip shows. */
const STRIP_DAYS = 14;

const days = (n: number) => `${n} ${n === 1 ? "day" : "days"}`;
const percent = (part: number, whole: number) => `${whole ? Math.round((part / whole) * 100) : 0}%`;
const ratio = (part: number, whole: number) => (whole ? (part / whole).toFixed(1) : "0");

/** A folder by its last segment; Chat mode's chats have none. */
const projectName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? "Chat mode";

const hourLabel = (h: number) => `${String(h).padStart(2, "0")}:00`;

function Cell({ day, at }: { day: UsageDay; at?: CSSProperties }) {
  return (
    <span
      role="gridcell"
      style={at}
      className={`usage-cell level-${day.level}`}
      title={
        day.tokens
          ? `${shortDay(day.day)}: ${formatTokens(day.tokens)} tokens, ${formatTokens(day.cached)} cached, ${formatTokens(day.completion)} output`
          : `${shortDay(day.day)}: no tokens`
      }
    />
  );
}

/**
 * The new agent chat's glance: one strip of the last fourteen days and one
 * line for them. Nothing at all until something has been counted, and no
 * error line on a page that is about something else.
 */
export function UsageStrip() {
  const { rows } = useUsage();
  const strip = useMemo(() => usageStrip(rows ?? [], new Date(), STRIP_DAYS), [rows]);
  const stats = useMemo(() => usageStats(rows ?? [], "14", new Date()), [rows]);
  if (!rows?.length) return null;

  const line = [
    `${formatTokens(stats.total)} tokens`,
    `${stats.sessions} ${stats.sessions === 1 ? "session" : "sessions"}`,
    stats.favoriteModel,
    stats.currentStreak > 1 ? `${stats.currentStreak}-day streak` : null,
  ].filter(Boolean);
  return (
    <div className="usage-glance">
      <div className="usage-strip" role="grid" aria-label="Tokens per day">
        <div role="row">
          {strip.map((day) => (
            <Cell key={day.day} day={day} />
          ))}
        </div>
      </div>
      <p className="usage-line">
        <span>Last {STRIP_DAYS} days</span>
        {line.map((part) => (
          <span key={part}>{part}</span>
        ))}
      </p>
    </div>
  );
}

/** A card of the Usage pane: a small uppercase title, then its body. */
function Card({ title, aside, children }: { title: string; aside?: string; children: ReactNode }) {
  return (
    <section className="usage-card">
      <h4 className="usage-card-title">
        {title}
        {aside && <span>{aside}</span>}
      </h4>
      {children}
    </section>
  );
}

/** The leaders of one counter, each with its share of the whole as a bar. */
function Ranking({ items, name, figure }: { items: Ranked[]; name: (key: string) => string; figure: (item: Ranked, total: number) => string }) {
  const total = items.reduce((sum, i) => sum + i.value, 0);
  if (!items.length) return <p className="usage-none">Nothing yet</p>;
  return (
    <ol className="usage-ranking">
      {items.map((item) => (
        <li key={item.key} title={item.key || name(item.key)}>
          <div>
            <span>{name(item.key)}</span>
            <b>{figure(item, total)}</b>
          </div>
          <i style={{ width: percent(item.value, items[0].value) }} />
        </li>
      ))}
    </ol>
  );
}

/**
 * Settings → Usage, after Claude Code's `/stats`: the headline figures for
 * the range picked beside the title, the last half year as GitHub draws
 * commits, then which models, which hours, which folders and which tools.
 */
export function UsagePane() {
  const { rows, error } = useUsage();
  const [range, setRange] = useState<UsageRange>("all");
  const map = useMemo(() => usageHeatmap(rows ?? [], new Date()), [rows]);
  const stats = useMemo(() => usageStats(rows ?? [], range, new Date()), [rows, range]);
  const peak = stats.hours.indexOf(Math.max(...stats.hours));

  // Label, figure, the line under it, and what hovering says beyond that.
  const tiles: [string, string, string, string?][] = [
    ["Tokens", formatTokens(stats.total), `${percent(stats.cached, stats.total)} cached`, `${formatTokens(stats.completion)} of them output`],
    ["Sessions", `${stats.sessions}`, `${days(stats.activeDays)} active of ${stats.days}`],
    ["Prompts", `${stats.prompts}`, `${ratio(stats.prompts, stats.sessions)} per session`],
    ["Tool calls", `${stats.toolCalls}`, `${ratio(stats.toolCalls, stats.prompts)} per prompt`],
  ];
  const activity: [string, string][] = [
    ["Current streak", days(stats.currentStreak)],
    ["Longest streak", days(stats.longestStreak)],
    ["Most active day", stats.mostActive ? shortDay(stats.mostActive) : "—"],
    ["Per active day", formatTokens(stats.activeDays ? Math.round(stats.total / stats.activeDays) : 0)],
  ];

  return (
    <div className="usage">
      <header className="usage-header">
        <h3 className="usage-title">Usage</h3>
        <div className="usage-ranges" role="radiogroup" aria-label="Range">
          {RANGES.map(({ value, label }) => (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={range === value}
              className={`usage-range${range === value ? " active" : ""}`}
              onClick={() => setRange(value)}
            >
              {label}
            </button>
          ))}
        </div>
      </header>

      {error && <p className="usage-error">{error}</p>}

      <dl className="usage-tiles">
        {tiles.map(([label, value, note, more]) => (
          <div key={label} className="usage-tile" title={more}>
            <dt>{label}</dt>
            <dd>{value}</dd>
            <dd className="usage-tile-note">{note}</dd>
          </div>
        ))}
      </dl>

      <Card title="Activity" aside={`Last ${HEATMAP_WEEKS} weeks`}>
        {/* One grid across the card: a column of weekday names, then a
            column per week sharing the rest, square cells. */}
        <div
          className="usage-map"
          role="grid"
          aria-label="Tokens per day"
          style={{ gridTemplateColumns: `auto repeat(${map.weeks.length}, minmax(0, 1fr))` }}
        >
          {map.months.map(
            (month, i) =>
              month && (
                <span key={`m${i}`} className="usage-month" aria-hidden style={{ gridColumn: i + 2, gridRow: 1 }}>
                  {month}
                </span>
              ),
          )}
          {WEEKDAYS.map((name, j) => (
            <span key={`d${j}`} className="usage-weekday" aria-hidden style={{ gridColumn: 1, gridRow: j + 2 }}>
              {name}
            </span>
          ))}
          {map.weeks.map((week, i) => (
            <div className="usage-week" role="row" key={i}>
              {week.map((day, j) => day && <Cell key={j} day={day} at={{ gridColumn: i + 2, gridRow: j + 2 }} />)}
            </div>
          ))}
        </div>
        <div className="usage-legend" aria-hidden>
          Less
          {[0, 1, 2, 3, 4].map((level) => (
            <span key={level} className={`usage-cell level-${level}`} />
          ))}
          More
        </div>
        <dl className="usage-facts">
          {activity.map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </Card>

      <div className="usage-pair">
        <Card title="Models">
          <Ranking items={stats.topModels} name={(key) => key} figure={(m, total) => percent(m.value, total)} />
        </Card>
        <Card title="Peak hours" aside={stats.hours[peak] > 0 ? `${hourLabel(peak)}–${hourLabel((peak + 1) % 24)}` : undefined}>
          <div className="usage-bars" role="img" aria-label="Tokens by hour of the day">
            {stats.hours.map((tokens, h) => (
              <span
                key={h}
                className={h === peak && tokens > 0 ? "peak" : undefined}
                style={{ height: `${stats.hours[peak] ? Math.max((tokens / stats.hours[peak]) * 100, tokens ? 6 : 0) : 0}%` }}
                title={`${hourLabel(h)}: ${formatTokens(tokens)} tokens`}
              />
            ))}
          </div>
          <div className="usage-bar-labels" aria-hidden>
            <span>00</span>
            <span>06</span>
            <span>12</span>
            <span>18</span>
            <span>24</span>
          </div>
        </Card>
      </div>

      <div className="usage-pair">
        <Card title="Projects">
          <Ranking items={stats.topProjects} name={projectName} figure={(p, total) => percent(p.value, total)} />
        </Card>
        <Card title="Tools">
          <Ranking items={stats.topTools} name={(key) => key} figure={(t) => `${t.value}`} />
        </Card>
      </div>

      <p className="usage-note">
        Every request to the model counts — the chat's own and its helpers', titles, commit messages and summaries —
        by what the provider reports. A provider that reports nothing adds nothing.
      </p>
    </div>
  );
}
