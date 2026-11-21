import { Text, View } from "nsh";
import { Link, Progress } from "nsh/base";
import { defineWidget, WidgetAction, WidgetDate } from "nsh/widget";

export interface Holding {
  symbol: string;
  name: string;
  price: string;
  changePct: string;
  positive: boolean;
}

export interface PortfolioWidgetData {
  symbol: string;
  name: string;
  price: string;
  changePct: string;
  positive: boolean;
  goalProgress: number;
  updatedAt: number;
  watchlist: Holding[];
}

const DEFAULT_DATA: PortfolioWidgetData = {
  symbol: "AAPL",
  name: "Apple Inc.",
  price: "$218.40",
  changePct: "+1.42%",
  positive: true,
  goalProgress: 0.72,
  updatedAt: 1795280000000,
  watchlist: [
    { symbol: "AAPL", name: "Apple", price: "$218.40", changePct: "+1.42%", positive: true },
    { symbol: "NVDA", name: "NVIDIA", price: "$142.80", changePct: "+3.15%", positive: true },
    { symbol: "MSFT", name: "Microsoft", price: "$428.10", changePct: "-0.38%", positive: false },
  ],
};

export default defineWidget<PortfolioWidgetData>({
  placeholder: DEFAULT_DATA,
  url: ({ data }) => `pulse://symbol/${(data ?? DEFAULT_DATA).symbol}`,
  render({ data, family }) {
    const item = data ?? DEFAULT_DATA;
    const deltaClass = item.positive
      ? "text-xs font-semibold text-emerald-500"
      : "text-xs font-semibold text-destructive";

    return (
      <View class="flex-1 flex-col justify-between p-4 bg-card">
        <View class="flex-row items-center justify-between gap-2">
          <View class="flex-col">
            <Text class="text-sm font-semibold text-foreground">{item.symbol}</Text>
            {family !== "small" && (
              <Text class="text-xs text-muted-foreground">{item.name}</Text>
            )}
          </View>
          <WidgetAction
            action="refresh"
            payload={{ symbol: item.symbol }}
            accessibilityLabel={`Refresh ${item.symbol} quote`}
            class="px-2.5 py-1 rounded-md bg-secondary"
          >
            <Text class="text-xs font-medium text-secondary-foreground">Refresh</Text>
          </WidgetAction>
        </View>

        <View class="flex-col gap-1">
          <Text class="text-2xl font-bold text-foreground">{item.price}</Text>
          <Text class={deltaClass} widget={{ accentable: true }}>
            {item.changePct}
          </Text>
        </View>

        {family === "large" && (
          <View class="flex-col gap-2 py-2">
            {item.watchlist.map((row) => (
              <Link
                href={`pulse://symbol/${row.symbol}`}
                class="flex-row items-center justify-between px-2.5 py-1.5 rounded-lg bg-secondary"
              >
                <View class="flex-col">
                  <Text class="text-xs font-semibold text-foreground">{row.symbol}</Text>
                  <Text class="text-[11px] text-muted-foreground">{row.name}</Text>
                </View>
                <View class="flex-col items-end">
                  <Text class="text-xs font-semibold text-foreground">{row.price}</Text>
                  <Text
                    class={
                      row.positive
                        ? "text-[11px] font-medium text-emerald-500"
                        : "text-[11px] font-medium text-destructive"
                    }
                    widget={{ accentable: true }}
                  >
                    {row.changePct}
                  </Text>
                </View>
              </Link>
            ))}
          </View>
        )}

        {family !== "small" && (
          <View class="flex-col gap-2">
            <Progress
              value={Math.round(item.goalProgress * 100)}
              class="h-1.5 rounded-full bg-muted"
              indicatorClass="h-1.5 rounded-full bg-primary"
            />
            <View class="flex-row items-center justify-between">
              <Link href={`pulse://symbol/${item.symbol}/chart`} class="text-xs text-primary">
                <Text class="text-xs font-medium text-primary">Open Chart</Text>
              </Link>
              <WidgetDate
                date={item.updatedAt}
                mode="relative"
                class="text-xs text-muted-foreground"
              />
            </View>
          </View>
        )}
      </View>
    );
  },
});
