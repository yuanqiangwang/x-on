import { pinyin } from "pinyin-pro";

// ---------------------------------------------------------------------------
// 检索匹配深模块。
// 对外四个入口：index（重建每条目的拼音索引，每条目只拼音化一次）、setUsage
// （注入使用记录，换算成 frecency 权重）、search（排序返回，只消费排好序的结果，
// 不暴露 tier）与 frequent（空态常用列表）。全部匹配/排序/噪音控制/同音抑制都藏在
// 模块内部 —— 改匹配策略只动这一个文件（locality）。
// 权重与拼音化一样**由外部注入**，matcher 不自己读存储：保持纯函数，测试可喂假数据。
//
// 核心：查询与条目走同一套拼音化。dd / dingding / 钉钉 / 丁丁 都能命中
// "钉钉"；且「字面命中排他」保证打对字就不带出同音候选。
// ---------------------------------------------------------------------------

export type AppInfo = {
  name: string;
  comment?: string | null;
  launchPath: string;
  targetPath: string;
  icon?: string | null;
  /** 额外的检索名（如系统工具的英文原生名 "Control Panel"）：参与匹配，并在列表里作副标题显示。 */
  aliases?: string[];
};

/** 一条使用记录：启动次数 + 最近一次启动时间（Unix 毫秒）。 */
export type UsageEntry = { count: number; lastUsed: number };
/** 全量使用记录快照：`launchPath -> UsageEntry`，来自后端 `get_usage`。 */
export type UsageMap = Record<string, UsageEntry>;

/** 一个 CJK 连续段 -> 无声调小写音节数组。唯一接触拼音库的点，测试可注入确定性输出。 */
export type Syllabize = (cjkRun: string) => string[];

type PinyinFields = { full: string; init: string; words: string };

// CJK 字区：Unified、Extension A、Compatibility Ideographs。
const CJK_CLASS = `一-鿿㐀-䶿豈-﫿`;
const CJK_RE = new RegExp(`[${CJK_CLASS}]`);
const isCJK = (ch: string) => CJK_RE.test(ch);

/** 一条文本 -> 检索键：full 全拼拼接、init 音节首字母、words 多词词首缩写。 */
function computePinyin(name: string, syllabize: Syllabize): PinyinFields {
  const runs =
    name.match(new RegExp(`[${CJK_CLASS}]+|[^${CJK_CLASS}]+`, "g")) ?? [name];
  let full = "";
  let init = "";
  for (const seg of runs) {
    if (isCJK(seg[0])) {
      for (const syl of syllabize(seg)) {
        full += syl;
        init += syl[0] ?? "";
      }
    } else {
      for (const ch of seg) {
        if (/\s/.test(ch)) continue; // 丢掉空格，多词前缀才能干净匹配
        const lc = ch.toLowerCase();
        full += lc;
        init += lc;
      }
    }
  }
  return { full, init, words: wordInitials(name, syllabize) };
}

// 空白隔开每个 token 的首字母缩写（Google Chrome -> gc）；单 token 名字为空。
function wordInitials(name: string, syllabize: Syllabize): string {
  const tokens = name.split(/[\s　-]+/).filter(Boolean);
  if (tokens.length < 2) return "";
  let out = "";
  for (const w of tokens) {
    const first = w[0] ?? "";
    out += isCJK(first)
      ? (syllabize(first)[0]?.charAt(0) ?? first.charAt(0)).toLowerCase()
      : first.toLowerCase();
  }
  return out;
}

// 查询侧：直接复用 computePinyin —— 对纯 ASCII 它走非 CJK 分支（full=init=
// 小写原文、不调拼音库），即"ASCII 折叠零成本"；含中文才派生拼音。
function queryKeys(t: string, syllabize: Syllabize): { full: string; init: string } {
  const p = computePinyin(t, syllabize);
  return { full: p.full, init: p.init };
}

// 子序列：q 是否按顺序、允许间隙地出现在 s 里。
function isSubsequence(q: string, s: string): boolean {
  let i = 0;
  for (const c of s) {
    if (c === q[i]) i++;
    if (i === q.length) return true;
  }
  return i === q.length;
}

// 拼音派生键的前缀门槛：长度过短的 needle 要求精确相等，堵住单字母模糊
// 前缀让结果集爆炸（原始 name 前缀不设此网关）。
const MIN_PREFIX = 2;
function prefixOrExact(hay: string, needle: string): boolean {
  if (needle.length < MIN_PREFIX) return hay === needle;
  return hay.startsWith(needle);
}

// ---------------------------------------------------------------------------
// frecency 权重（ADR 0002）：分桶衰减 × log2(次数)。
// 用分桶而不是连续指数衰减，是为了让权重大小可读、可手改，且不受系统时钟回拨影响。
//
// 权重**只在 tier 与 len 都相同时**才参与比较（见 search 的排序键）—— 任何个人习惯
// 都不能把精确命中（tier 1）或名称前缀（tier 2）挤下去。
// ---------------------------------------------------------------------------
const DAY_MS = 24 * 60 * 60 * 1000;
const RECENCY_BUCKETS: { withinMs: number; score: number }[] = [
  { withinMs: DAY_MS, score: 100 }, // 今天
  { withinMs: 7 * DAY_MS, score: 80 }, // 本周
  { withinMs: 30 * DAY_MS, score: 60 }, // 本月
  { withinMs: Number.POSITIVE_INFINITY, score: 40 }, // 更早
];

/** 一条使用记录 -> 排序权重。`now` 可注入，单测能把分桶钉死在确定的时间点上。 */
function frecencyWeight(u: UsageEntry, now: number): number {
  const age = Math.max(0, now - u.lastUsed); // 时钟回拨时按"刚刚"算，不产生负权重
  const bucket =
    RECENCY_BUCKETS.find((b) => age <= b.withinMs) ??
    RECENCY_BUCKETS[RECENCY_BUCKETS.length - 1];
  return bucket.score * Math.log2(u.count + 1);
}

type Scored = { app: AppInfo; tier: number; len: number; literal: boolean; weight: number };
/** 打分层能给出的部分：条目与权重由调用方在外部补上。 */
type ScoredFields = Omit<Scored, "app" | "weight">;
// literal = 命中用查询**原始串**对条目**名字**；false = 用拼音派生键（full/init/words）。

export class AppMatcher {
  private syllabize: Syllabize;
  private apps: AppInfo[] = [];
  private idx = new Map<string, PinyinFields[]>(); // launchPath -> [显示名字段, ...别名字段]
  /** launchPath -> frecency 权重（由 setUsage 注入，不在这里读存储）。 */
  private weights = new Map<string, number>();
  /** 按权重降序的条目快照，`frequent` 直接切片。index/setUsage 时各重算一次。 */
  private ranked: AppInfo[] = [];

  constructor(syllabize?: Syllabize) {
    this.syllabize =
      syllabize ??
      ((run) =>
        (pinyin(run, { type: "all", toneType: "none" }) as { pinyin: string }[])
          .map((it) => (it.pinyin || run).toLowerCase()));
  }

  /** 替换 app 列表并重建索引。显示名 + 每个别名都在这里拼音化一次。 */
  index(apps: AppInfo[]): void {
    this.apps = apps;
    this.idx.clear();
    for (const a of apps) {
      // 数组与 [name, ...aliases] 严格对齐（不为空别名做裁剪，保证 score 能对应上 raw）。
      const fields: PinyinFields[] = [computePinyin(a.name, this.syllabize)];
      for (const alias of a.aliases ?? []) {
        fields.push(computePinyin(alias, this.syllabize));
      }
      this.idx.set(a.launchPath, fields);
    }
    this.rank();
  }

  /**
   * 注入使用记录，换算成 frecency 权重。
   *
   * 权重从外部注入而不是在这里读存储 —— matcher 保持纯函数，单测可直接喂一份假数据
   * （与 `Syllabize` 同一思路）。`now` 同样可注入，让分桶衰减在测试里是确定的。
   */
  setUsage(usage: UsageMap, now: number = Date.now()): void {
    this.weights.clear();
    for (const [path, u] of Object.entries(usage ?? {})) {
      if (u && u.count > 0) this.weights.set(path, frecencyWeight(u, now));
    }
    this.rank();
  }

  /** 按权重降序重排一份快照（`frequent` 用）。sort 是稳定的，权重为 0 的条目保持
   *  索引序 —— 没有任何使用数据时，`frequent` 的结果与接权重之前完全一致。 */
  private rank(): void {
    this.ranked = [...this.apps].sort(
      (a, b) => (this.weights.get(b.launchPath) ?? 0) - (this.weights.get(a.launchPath) ?? 0),
    );
  }

  /** 排序返回。空/全空白查询返回全部（原顺序）；无匹配返回 []。 */
  search(query: string): AppInfo[] {
    const t = query.trim().toLowerCase();
    if (!t) return this.apps;
    const q = queryKeys(t, this.syllabize);

    const literal: Scored[] = [];
    const pinyin: Scored[] = [];
    for (const a of this.apps) {
      const s = this.score(a, t, q);
      if (s) {
        const item: Scored = { app: a, ...s, weight: this.weights.get(a.launchPath) ?? 0 };
        (s.literal ? literal : pinyin).push(item);
      }
    }

    // 字面命中排他——仅对**含中文**的查询生效：打对字 = 意图明确，抑制同音候选，
    // 不让同音喧宾夺主（打"钉钉"只出钉钉）。纯字母/拼音查询天然模糊，不抑制：
    // 字面命中置顶、拼音命中也保留并列，避免"wx"因某应用名含 "wx" 字面命中、
    // 而把微信（首字母 wx 命中）挤掉。
    const pool = CJK_RE.test(t) && literal.length ? literal : [...literal, ...pinyin];
    // 排序键（ADR 0002）：tier → len → frecency。权重只在末级参与 —— 它只能在同层
    // 同长的并列里破平，永远不会把精确命中或名称前缀挤下去。
    pool.sort((x, y) => x.tier - y.tier || x.len - y.len || y.weight - x.weight);
    return pool.map((s) => s.app);
  }

  /**
   * 空态常用列表：按 ↓ 展开时的候选来源，最多 `limit` 条，按 frecency 权重降序。
   *
   * 没有任何使用数据时权重全为 0，稳定排序退化成索引序 —— 与"接权重之前"完全一致。
   * 排序策略属于本模块，所以入口放这里而不是 main.ts。
   */
  frequent(limit: number): AppInfo[] {
    return this.ranked.slice(0, Math.max(0, limit));
  }

  // 7 层对称打分，tier 越小越强；同 tier 内 len 越小（查询覆盖字段比例越高）越优。
  // 对显示名 + 每个别名分别打分，取最优——这样中文「控制面板」和英文「control panel」
  // 都能命中同一个条目。
  private score(a: AppInfo, t: string, q: { full: string; init: string }): ScoredFields | null {
    const fields = this.idx.get(a.launchPath);
    if (!fields || fields.length === 0) return null;

    const raws = [a.name, ...(a.aliases ?? [])]; // 与 index 的 fields 数组对齐
    let best: ScoredFields | null = null;
    for (let i = 0; i < fields.length; i++) {
      const s = this.scoreEntry(raws[i] ?? a.name, fields[i], t, q);
      if (s && (best === null || s.tier < best.tier || (s.tier === best.tier && s.len < best.len))) {
        best = s;
      }
    }
    return best;
  }

  /** 对单条原始名（显示名或某个别名）及其拼音字段打 7 层分。literal 用该条 raw 判定。 */
  private scoreEntry(raw: string, f: PinyinFields, t: string, q: { full: string; init: string }): ScoredFields | null {
    const name = raw.toLowerCase();
    const full = f.full || name;
    const init = f.init || "";
    const words = f.words || "";

    if (name === t) return { tier: 1, len: name.length, literal: true };
    if (name.startsWith(t)) return { tier: 2, len: name.length, literal: true };
    if (prefixOrExact(full, q.full)) return { tier: 3, len: full.length, literal: false };
    if (words && prefixOrExact(words, t)) return { tier: 4, len: words.length, literal: false };
    if (init && prefixOrExact(init, q.init)) return { tier: 5, len: init.length, literal: false };

    const inName = name.includes(t);
    const inFull = full.includes(t);
    if (inName || inFull) {
      const len = Math.min(inName ? name.length : Infinity, inFull ? full.length : Infinity);
      return { tier: 6, len, literal: inName };
    }

    const seqName = isSubsequence(t, name);
    const seqFull = isSubsequence(t, full);
    if (seqName || seqFull) {
      const len = Math.min(seqName ? name.length : Infinity, seqFull ? full.length : Infinity);
      return { tier: 7, len, literal: seqName };
    }
    return null;
  }
}
