//! Missing samples and collection gaps stay gaps in the responsive SVG history.
use super::{Metric, State};
use json_codec::Value;
use wasm_bindgen::{JsCast, closure::Closure};
use web_interface::{html, i18n::t, prelude::*};

#[derive(Debug)]
struct Plot {
    paths: Vec<String>,
    dots: Vec<(f64, f64)>,
    maximum: f64,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}
fn plot(
    points: &[(f64, Option<f64>)],
    from: f64,
    to: f64,
    interval: f64,
    width: f64,
    height: f64,
    is_power: bool,
) -> Plot {
    let maximum = if is_power {
        points
            .iter()
            .filter_map(|(_, v)| *v)
            .filter(|v| v.is_finite() && *v >= 0.)
            .fold(1.0f64, f64::max)
            * 1.1
    } else {
        100.
    };
    let left = 38.0f64.min(width * 0.35);
    let right = (width - 5.).max(left + 1.);
    let top = 8.0f64.min(height * 0.2);
    // Short four-column cards retain useful graph height and the same font.
    // Their selected time range is already shown by the toolbar control.
    let bottom = (height - if height < 70. { 6. } else { 24. }).max(top + 1.);
    let mut paths = Vec::new();
    let mut dots = Vec::new();
    let mut segment = Vec::new();
    let mut previous_time = None;
    let flush = |segment: &mut Vec<(f64, f64)>,
                 paths: &mut Vec<String>,
                 dots: &mut Vec<(f64, f64)>| {
        if segment.len() == 1 {
            dots.push(segment[0]);
        } else if !segment.is_empty() {
            paths.push(
                segment
                    .iter()
                    .enumerate()
                    .map(|(i, (x, y))| format!("{}{x:.3},{y:.3}", if i == 0 { "M" } else { "L" }))
                    .collect::<String>(),
            );
        }
        segment.clear();
    };
    for (timestamp, value) in points {
        let value = value.filter(|v| v.is_finite() && *v >= 0. && *v <= maximum);
        if !timestamp.is_finite() || *timestamp < from || *timestamp > to || value.is_none() {
            flush(&mut segment, &mut paths, &mut dots);
            previous_time = None;
            continue;
        }
        if previous_time.is_some_and(|previous| {
            *timestamp <= previous || *timestamp - previous > interval.max(1.) * 2.5
        }) {
            flush(&mut segment, &mut paths, &mut dots);
        }
        let x = left + ((*timestamp - from) / (to - from).max(1.)).clamp(0., 1.) * (right - left);
        let y = bottom
            - value.expect("invariant: plotted sample passed value validation") / maximum
                * (bottom - top);
        segment.push((x, y));
        previous_time = Some(*timestamp);
    }
    flush(&mut segment, &mut paths, &mut dots);
    Plot {
        paths,
        dots,
        maximum,
        left,
        right,
        top,
        bottom,
    }
}
fn milliseconds(value: &Value) -> Option<f64> {
    value
        .as_str()
        .map(js_sys::Date::parse)
        .filter(|value| value.is_finite())
}
fn axis_max(value: f64) -> String {
    if value >= 1000. {
        format!("{:.1}k", value / 1000.)
    } else if value >= 10. {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Series {
    points: Vec<(f64, Option<f64>)>,
    from: f64,
    to: f64,
    interval: f64,
}
fn series(history: &Value, metric: &Metric, parse: fn(&Value) -> Option<f64>) -> Series {
    Series {
        points: history["samples"]
            .as_array()
            .map(|samples| {
                samples
                    .iter()
                    .filter_map(|sample| {
                        Some((
                            parse(&sample["collected_at"])? / 1000.,
                            metric.history_value(sample),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        from: parse(&history["from"]).unwrap_or(0.) / 1000.,
        to: parse(&history["to"]).unwrap_or(0.) / 1000.,
        interval: history["interval_seconds"].as_f64().unwrap_or(5.),
    }
}
fn series_projection(
    state: RwSignal<State>,
    metric: Metric,
    parse: fn(&Value) -> Option<f64>,
) -> Memo<Series> {
    let history = Memo::new(move |_| state.with(|s| s.monitor.history.clone()));
    Memo::new(move |_| history.with(|history| series(history, &metric, parse)))
}

pub(super) fn view(state: RwSignal<State>, metric: Metric) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let dimensions = RwSignal::new((280.0f64, 110.0f64));
    root.on_load(move |element| {
        let element: web_sys::Element = element.unchecked_into();
        let update = move || {
            let b = element.get_bounding_client_rect();
            let next = (b.width().max(1.), b.height().max(1.));
            if dimensions
                .with_untracked(|old| (old.0 - next.0).abs() > 0.2 || (old.1 - next.1).abs() > 0.2)
            {
                dimensions.set(next);
            }
        };
        update();
        let callback = Closure::wrap(Box::new(
            move |_: js_sys::Array, _: web_sys::ResizeObserver| update(),
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>);
        match web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) {
            Ok(observer) => {
                if let Some(element) = root.get_untracked() {
                    observer.observe(&element);
                }
                let cleanup = send_wrapper::SendWrapper::new((observer, callback));
                on_cleanup(move || {
                    cleanup.0.disconnect();
                    drop(cleanup);
                });
            }
            Err(error) => {
                web_sys::console::warn_2(&"Could not observe chart resizing".into(), &error)
            }
        }
    });
    let label = metric.label();
    let aria_label = format!("{} {}", label, t("历史图表", "history chart"));
    let is_power = metric.is_power();
    let samples = series_projection(state, metric, milliseconds);
    let presentation = Memo::new(move |_| {
        state.with(|s| {
            (
                s.monitor.history_loading,
                s.monitor.history_loaded,
                s.monitor.range.clone(),
            )
        })
    });
    view! {<div class="zxs-metric-chart" node_ref=root aria-label=aria_label>
        {move ||{
            let (width,height)=dimensions.get();
            let (loading,loaded,range)=presentation.get();
            let plot=samples.with(|series| plot(&series.points,series.from,series.to,series.interval,width,height,is_power));
            if plot.paths.is_empty()&&plot.dots.is_empty(){return view!{<p class="zxs-chart-empty">{if loading&&!loaded{t("正在读取历史…","Loading history…")}else{t("暂无可用历史","No available history yet")}}</p>}.into_any();}
            let axis_y=(height-4.).max(14.4);
            view!{<svg viewBox=format!("0 0 {width} {height}") preserveAspectRatio="none" role="img" aria-label=format!("{} · {}",label,range)>
                <title>{format!("{} · {} · {}",label,range,t("空缺表示未采样或不可用，不表示零。","Gaps indicate missing or unavailable samples, not zero."))}</title>
                <path class="zxs-chart-grid" d=format!("M{} {}H{}M{} {}H{}",plot.left,plot.top,plot.right,plot.left,plot.bottom,plot.right)/>
                <text class="zxs-chart-label" x="0" y=(plot.top+5.).to_string()>{axis_max(plot.maximum)}</text>{(height>=42.).then(||view!{<text class="zxs-chart-label" x="0" y=plot.bottom.to_string()>"0"</text>})}
                {(height>=70.).then(||view!{<text class="zxs-chart-label" x=plot.left.to_string() y=axis_y.to_string()>{format!("−{range}")}</text><text class="zxs-chart-label" x=plot.right.to_string() y=axis_y.to_string() text-anchor="end">{t("现在","Now")}</text>})}
                {plot.paths.into_iter().map(|path|view!{<path class="zxs-chart-line" d=path/>}).collect_view()}
                {plot.dots.into_iter().map(|(x,y)|view!{<circle class="zxs-chart-dot" cx=x.to_string() cy=y.to_string() r="2"/>}).collect_view()}
            </svg>}.into_any()
        }}
    </div>}
}
#[cfg(test)]
#[path = "../../../../test/web/unit/monitor/chart/tests.rs"]
mod tests;
