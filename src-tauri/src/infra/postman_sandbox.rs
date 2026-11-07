//! Postman's own `pm` in QuickJS (`docs/45-http-client.md`, decision 8 and HC-0 b):
//! `postman-sandbox`'s bootcode, driven through uvm's bridge as plain calls. A script
//! has no files, no sockets and no window — only what `pm` gives it, and its one way
//! out, `pm.sendRequest`, is the `requests` callback. The sandbox lives for one send,
//! on that thread: a QuickJS runtime is not `Send`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rquickjs::context::EvalOptions;
use rquickjs::{CatchResultExt, Coerced, Context, Ctx, FromJs, Function, Object, Persistent, Runtime};
use serde_json::{json, Value};

use crate::domain::http_client::HttpError;
use crate::domain::http_scripts::{ConsoleLine, TestResult};
use crate::domain::postman::text_of;

const BOOTCODE: &str = include_str!("../../postman-sandbox/bootcode.js");
const BRIDGE: &str = include_str!("../../postman-sandbox/bridge-client.js");
const FLATTED: &str = include_str!("../../postman-sandbox/flatted.js");
const PRELUDE: &str = include_str!("postman_prelude.js");

/// A script's heap; the sandbox itself takes about 18 MB of it.
const MEMORY: usize = 256 << 20;
const STACK: usize = 1 << 20;
/// Past this a script's lines are not kept, and it goes on running. A loop that never
/// yields is only counted at its end: until then its lines wait in `out`.
const CONSOLE_LINES: usize = 500;
const CONSOLE_LINE: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// JavaScript running without a stop: past it, `while (true) {}` is interrupted.
    pub running: Duration,
    /// A script from start to end, its timers and `pm.sendRequest` included: a pause
    /// of a few seconds before the next step is what many scripts are for (HC-0).
    pub whole: Duration,
}

/// Shorter under test: a send's loop without end is tested through the real path.
pub const LIMITS: Limits = if cfg!(test) {
    Limits { running: Duration::from_secs(2), whole: Duration::from_secs(10) }
} else {
    Limits { running: Duration::from_secs(10), whole: Duration::from_secs(60) }
};

/// What a script left. `error` is a script's own failure; its variables still stand.
#[derive(Debug, Default)]
pub struct Ran {
    /// The execution: its scopes and its `request`.
    pub result: Value,
    pub tests: Vec<TestResult>,
    pub console: Vec<ConsoleLine>,
    pub error: Option<String>,
    /// `pm.execution.skipRequest()` was called.
    pub skip_request: bool,
}

// Fields drop in this order, and `Persistent` and `Context` must go before `Runtime`.
pub struct Sandbox {
    host: Persistent<Object<'static>>,
    context: Context,
    runtime: Runtime,
    out: Rc<RefCell<Vec<String>>>,
    rejected: Rc<RefCell<Vec<Value>>>,
    until: Arc<Mutex<Instant>>,
    limits: Limits,
    runs: Cell<u32>,
}

fn failed(why: impl std::fmt::Display) -> HttpError {
    HttpError::Script(why.to_string())
}

fn js<T>(ctx: &Ctx<'_>, result: rquickjs::Result<T>) -> Result<T, HttpError> {
    result.catch(ctx).map_err(failed)
}

/// The bridge writes globals it never declares, so nothing here is strict.
fn eval<'js, T: rquickjs::FromJs<'js>>(ctx: &Ctx<'js>, source: impl Into<Vec<u8>>) -> Result<T, HttpError> {
    let mut options = EvalOptions::default();
    options.strict = false;
    js(ctx, ctx.eval_with_options(source, options))
}

impl Sandbox {
    pub fn boot(limits: Limits) -> Result<Sandbox, HttpError> {
        let runtime = Runtime::new().map_err(failed)?;
        runtime.set_memory_limit(MEMORY);
        runtime.set_max_stack_size(STACK);
        let until = Arc::new(Mutex::new(Instant::now() + LIMITS.whole));
        let watched = until.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > *watched.lock().unwrap_or_else(PoisonError::into_inner))));
        let rejected = Rc::new(RefCell::new(Vec::new()));
        let tracked = rejected.clone();
        // ponytail: a rejection handled late takes the newest off the list, which can
        // name the wrong one of several; the count stays right, and only the text differs.
        runtime.set_host_promise_rejection_tracker(Some(Box::new(move |ctx, _, reason, handled| {
            let mut tracked = tracked.borrow_mut();
            if handled {
                tracked.pop();
            } else {
                tracked.push(rejection(&ctx, &reason));
            }
        })));
        let context = Context::full(&runtime).map_err(failed)?;
        let out = Rc::new(RefCell::new(Vec::new()));
        let sink = out.clone();
        let host = context.with(|ctx| -> Result<_, HttpError> {
            let flatted: Object = eval(&ctx, format!("(function(self){{{FLATTED}\n;return self.Flatted}})({{}})"))?;
            let prelude: Function = eval(&ctx, PRELUDE)?;
            let emit = js(&ctx, Function::new(ctx.clone(), move |s: String| sink.borrow_mut().push(s)))?;
            let host: Object = js(&ctx, prelude.call((ctx.globals(), flatted, emit)))?;
            eval::<()>(&ctx, BRIDGE)?;
            let dispatch: Function = js(&ctx, ctx.globals().get("__uvm_dispatch"))?;
            let bind: Function = js(&ctx, host.get("bind"))?;
            js(&ctx, bind.call::<_, ()>((dispatch,)))?;
            js(&ctx, ctx.globals().remove("__uvm_dispatch"))?;
            eval::<()>(&ctx, BOOTCODE)?;
            Ok(Persistent::save(&ctx, host))
        })?;
        let sandbox = Sandbox { host, context, runtime, out, rejected, until, limits, runs: Cell::new(0) };
        sandbox.send("initialize", json!([{}]))?;
        sandbox.pump(&mut |_| Ok(()), "initialize", Instant::now() + LIMITS.whole)?;
        Ok(sandbox)
    }

    /// The script `code` to its end. `requests` is `pm.sendRequest`: a Postman request
    /// in, what the callback gets or why it failed out. Past a limit it is `Err`, and
    /// the sandbox is not used again: what was interrupted may be half done.
    pub fn run(&self, listen: &str, code: &str, name: &str, context: Value, requests: &mut dyn FnMut(&Value) -> Result<Value, HttpError>) -> Result<Ran, HttpError> {
        let id = format!("run-{}", self.runs.replace(self.runs.get() + 1));
        let event = json!({"listen": listen, "script": {"type": "text/javascript", "exec": code.lines().collect::<Vec<_>>()}});
        let options = json!({"cursor": {"ref": id}, "legacy": {"_itemName": name}, "resolvedPackages": {}});
        let result_event = format!("execution.result.{id}");
        let mut ran = Ran::default();
        self.send("execute", json!([id, event, context, options]))?;
        self.pump(
            &mut |message| {
                let name = message[0].as_str().unwrap_or_default();
                if name == "execution.assertion" {
                    ran.tests.extend(message[2].as_array().into_iter().flatten().map(test_result));
                } else if name == "execution.console" {
                    if ran.console.len() < CONSOLE_LINES {
                        ran.console.push(console_line(message));
                    }
                } else if name == "execution.error" {
                    ran.error.get_or_insert(error_text(&message[2]));
                } else if name.starts_with("execution.skipRequest.") {
                    ran.skip_request = true;
                } else if let Some(run) = name.strip_prefix("execution.request.") {
                    let answer = match requests(&message[4]) {
                        Ok(response) => json!([message[3], null, response, null]),
                        Err(why) => json!([message[3], {"name": "Error", "message": why.to_string()}, null, null]),
                    };
                    self.send(&format!("execution.response.{run}"), answer)?;
                } else if name == result_event {
                    if !message[1].is_null() {
                        ran.error = Some(error_text(&message[1]));
                    }
                    ran.result = message[2].clone();
                }
                Ok(())
            },
            &result_event,
            Instant::now() + self.limits.whole,
        )?;
        Ok(ran)
    }

    fn arm(&self) {
        *self.until.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now() + self.limits.running;
    }

    fn call<T: for<'js> rquickjs::FromJs<'js>>(&self, method: &str, args: Option<(&str, String)>) -> Result<T, HttpError> {
        self.arm();
        self.context.with(|ctx| {
            let host = self.host.clone().restore(&ctx).map_err(failed)?;
            let function: Function = js(&ctx, host.get(method))?;
            match args {
                Some(args) => js(&ctx, function.call((args.0, args.1))),
                None => js(&ctx, function.call(())),
            }
        })
        .map_err(out_of_time)
    }

    fn send(&self, name: &str, args: Value) -> Result<(), HttpError> {
        self.call("send", Some((name, args.to_string())))
    }

    /// Jobs, messages and timers, in the real time they take, until `end` has been handled.
    fn pump(&self, handle: &mut dyn FnMut(&Value) -> Result<(), HttpError>, end: &str, deadline: Instant) -> Result<(), HttpError> {
        loop {
            self.arm();
            while self.runtime.is_job_pending() {
                self.runtime.execute_pending_job().map_err(|e| out_of_time(failed(e.0.with(|ctx| format!("{:?}", ctx.catch())))))?;
            }
            // What a browser's `unhandledrejection` says to uvm: an error in a
            // `pm.sendRequest` callback, which runs inside a promise, arrives this way.
            let rejected: Vec<Value> = self.rejected.borrow_mut().drain(..).collect();
            for reason in rejected {
                self.send("uncaughtException", json!([reason]))?;
            }
            let messages: Vec<String> = self.out.borrow_mut().drain(..).collect();
            let quiet = messages.is_empty();
            let mut ended = false;
            for message in messages {
                // Here and not after them: each `pm.sendRequest` among them can take its own 30 seconds.
                if Instant::now() > deadline {
                    return Err(failed("a script ran out of time"));
                }
                let message: Value = serde_json::from_str(&message).map_err(failed)?;
                handle(&message)?;
                ended |= message[0].as_str().is_some_and(|name| name.starts_with(end));
            }
            if ended {
                return Ok(());
            }
            if !quiet {
                continue;
            }
            let due: f64 = self.call("due", None)?;
            if due < 0.0 {
                return Err(failed("a script ended without saying so"));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            // A script's delay can be `Infinity`, which no `Duration` holds.
            match Duration::try_from_secs_f64(due / 1000.0) {
                Ok(wait) if wait <= left => std::thread::sleep(wait),
                _ => {
                    std::thread::sleep(left);
                    return Err(failed("a script ran out of time"));
                }
            }
            self.call::<()>("fire", None)?;
        }
    }
}

/// An interrupted script reads as QuickJS's own error: said here in the page's words.
fn out_of_time(error: HttpError) -> HttpError {
    match &error {
        HttpError::Script(why) if why.contains("interrupted") => failed("a script ran out of time"),
        _ => error,
    }
}

/// A promise's rejection as the bridge carries an error. A getter that throws is the script's own.
fn rejection<'js>(ctx: &Ctx<'js>, reason: &rquickjs::Value<'js>) -> Value {
    let field = |name: &str| reason.as_object().and_then(|o| o.get::<_, String>(name).ok());
    let error = match (field("name"), field("message")) {
        (Some(name), Some(message)) => json!({"name": name, "message": message}),
        _ => json!({"name": "Error", "message": Coerced::<String>::from_js(ctx, reason.clone()).map(|text| text.0).unwrap_or_default()}),
    };
    drop(ctx.catch());
    error
}

fn test_result(assertion: &Value) -> TestResult {
    TestResult {
        name: text_of(assertion.get("name")),
        passed: assertion.get("passed") == Some(&Value::Bool(true)),
        skipped: assertion.get("skipped") == Some(&Value::Bool(true)),
        error: assertion.get("error").filter(|e| !e.is_null()).map(error_text),
    }
}

fn console_line(message: &Value) -> ConsoleLine {
    let table: Vec<Value> = message[3].as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let args = table.first().map(|root| revive(&table, root, 0)).unwrap_or_default();
    let args = args.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut text = args.iter().map(|a| text_of(Some(a))).collect::<Vec<_>>().join(" ");
    if text.len() > CONSOLE_LINE {
        let mut end = CONSOLE_LINE;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    ConsoleLine { level: text_of(message.get(2)), text }
}

/// A console's arguments out of `teleport-javascript`: one table, the first row the
/// root, and inside a row a string is the number of the row that holds the value.
fn revive(table: &[Value], row: &Value, depth: usize) -> Value {
    if depth > 16 {
        return Value::String("[…]".into());
    }
    let reference = |v: &Value| match v.as_str().and_then(|at| at.parse::<usize>().ok()).and_then(|at| table.get(at)) {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(found) => revive(table, found, depth + 1),
        None => v.clone(),
    };
    match row {
        Value::Array(items) => Value::Array(items.iter().map(reference).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, v)| (key.clone(), reference(v))).collect()),
        other => other.clone(),
    }
}

fn error_text(error: &Value) -> String {
    let (name, message) = (text_of(error.get("name")), text_of(error.get("message")));
    match (name.is_empty(), message.is_empty()) {
        (_, true) => text_of(Some(error)),
        (true, false) => message,
        (false, false) => format!("{name}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_requests(_: &Value) -> Result<Value, HttpError> {
        Err(failed("no network in this test"))
    }

    fn sandbox() -> Sandbox {
        Sandbox::boot(LIMITS).unwrap()
    }

    fn environment(ran: &Ran) -> Vec<(String, String)> {
        let values = ran.result["environment"]["values"].as_array().cloned().unwrap_or_default();
        values.iter().map(|v| (text_of(v.get("key")), text_of(v.get("value")))).collect()
    }

    fn context() -> Value {
        json!({"environment": {"values": [{"key": "base", "value": "https://a.test", "type": "any"}]}, "request": {"url": "https://a.test/x", "method": "GET"}})
    }

    #[test]
    fn a_script_reads_and_writes_variables_and_changes_the_request() {
        let ran = sandbox()
            .run("prerequest", "pm.environment.set('got', pm.environment.get('base') + '/y');\npm.request.headers.add({key: 'X-Trace', value: pm.info.requestName});", "Orders", context(), &mut no_requests)
            .unwrap();
        assert_eq!(ran.error, None);
        assert_eq!(environment(&ran), vec![("base".to_string(), "https://a.test".to_string()), ("got".to_string(), "https://a.test/y".to_string())]);
        assert_eq!(ran.result["request"]["header"], json!([{"key": "X-Trace", "value": "Orders"}]));
    }

    #[test]
    fn tests_pass_and_fail_by_name_and_the_console_is_kept() {
        let mut context = context();
        context["response"] = json!({"code": 200, "header": [], "body": "{\"id\":42}"});
        let code = "pm.test('status', () => pm.response.to.have.status(200));\npm.test('wrong', () => pm.expect(pm.response.json().id).to.eql(1));\nconsole.warn('seen', 7, {a: ['b']});";
        let ran = sandbox().run("test", code, "r", context, &mut no_requests).unwrap();
        assert_eq!(ran.tests.iter().map(|t| (t.name.as_str(), t.passed)).collect::<Vec<_>>(), vec![("status", true), ("wrong", false)]);
        assert!(ran.tests[1].error.as_deref().unwrap().contains("expected 42 to deeply equal 1"), "{:?}", ran.tests[1].error);
        assert_eq!(ran.console, vec![ConsoleLine { level: "warn".into(), text: "seen 7 {\"a\":[\"b\"]}".into() }]);
    }

    #[test]
    fn what_real_scripts_use_works() {
        let code = "const moment = require('moment'); const { v4 } = require('uuid'); const CryptoJS = require('crypto-js');
pm.environment.set('year', moment('2026-10-28').format('YYYY'));
pm.environment.set('uuid', v4().length);
pm.environment.set('picked', _.pick({a: 1}, 'a').a);
pm.environment.set('hmac', CryptoJS.HmacSHA256('b', 'k').toString().length);
pm.environment.set('fn', new Function('return 6 * 7')());
pm.environment.set('dyn', pm.variables.replaceIn('{{$guid}}').length);
postman.setNextRequest('next');
postman.setEnvironmentVariable('legacy', 'yes');";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.error, None);
        let env = environment(&ran);
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("year"), Some("2026"));
        assert_eq!(get("uuid"), Some("36"));
        assert_eq!(get("picked"), Some("1"));
        assert_eq!(get("hmac"), Some("64"));
        assert_eq!(get("fn"), Some("42"));
        assert_eq!(get("dyn"), Some("36"));
        assert_eq!(get("legacy"), Some("yes"));
    }

    #[test]
    fn a_timer_takes_the_time_it_says() {
        let started = Instant::now();
        let ran = sandbox().run("prerequest", "setTimeout(() => pm.environment.set('late', 'yes'), 150);", "r", context(), &mut no_requests).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(environment(&ran).contains(&("late".to_string(), "yes".to_string())));
    }

    #[test]
    fn a_timer_past_the_limit_is_not_waited_out() {
        let sandbox = Sandbox::boot(Limits { running: Duration::from_secs(10), whole: Duration::from_millis(100) }).unwrap();
        let started = Instant::now();
        let error = sandbox.run("prerequest", "setTimeout(() => {}, 60000);", "r", context(), &mut no_requests).unwrap_err();
        assert_eq!(error.to_string(), "a script ran out of time");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_loop_without_end_is_stopped() {
        let sandbox = Sandbox::boot(Limits { running: Duration::from_millis(100), whole: Duration::from_secs(5) }).unwrap();
        let started = Instant::now();
        assert_eq!(sandbox.run("prerequest", "while (true) {}", "r", context(), &mut no_requests).unwrap_err().to_string(), "a script ran out of time");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_promise_loop_without_end_is_stopped_too() {
        let sandbox = Sandbox::boot(Limits { running: Duration::from_millis(100), whole: Duration::from_secs(5) }).unwrap();
        let code = "const spin = () => Promise.resolve().then(spin); spin();";
        assert_eq!(sandbox.run("prerequest", code, "r", context(), &mut no_requests).unwrap_err().to_string(), "a script ran out of time");
    }

    #[test]
    fn a_script_that_throws_keeps_what_it_set_and_says_why() {
        let ran = sandbox().run("prerequest", "pm.environment.set('before', '1');\nthrow new TypeError('broken');", "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.error.as_deref(), Some("TypeError: broken"));
        assert!(environment(&ran).contains(&("before".to_string(), "1".to_string())));
    }

    #[test]
    fn send_request_goes_through_the_host_and_its_failure_reaches_the_callback() {
        let mut asked = Vec::new();
        let code = "pm.sendRequest({url: 'https://id.test/token', method: 'POST'}, (err, res) => { pm.environment.set('token', res.json().access_token); });
pm.sendRequest('https://down.test/', (err, res) => { pm.environment.set('why', err.message); });";
        let ran = sandbox()
            .run("prerequest", code, "r", context(), &mut |request| {
                asked.push(request.clone());
                if asked.len() == 1 {
                    Ok(json!({"code": 200, "header": [], "body": "{\"access_token\":\"t-1\"}"}))
                } else {
                    Err(failed("refused"))
                }
            })
            .unwrap();
        assert_eq!(asked[0]["method"], "POST");
        let env = environment(&ran);
        assert!(env.contains(&("token".to_string(), "t-1".to_string())), "{env:?}");
        assert!(env.contains(&("why".to_string(), "refused".to_string())), "{env:?}");
    }

    #[test]
    fn a_script_has_nothing_of_the_machine() {
        let code = "pm.environment.set('have', ['require(\"fs\").readFileSync(\"/etc/hosts\")', 'process.env', 'fetch', 'XMLHttpRequest', 'std', 'os', '__uvm_dispatch'].filter((n) => { try { return !!eval(n); } catch (e) { return false; } }).join());";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.error, None);
        assert!(environment(&ran).contains(&("have".to_string(), String::new())), "{:?}", environment(&ran));
    }

    #[test]
    fn scripts_of_one_send_share_a_sandbox_and_skipping_is_said() {
        let sandbox = sandbox();
        assert!(!sandbox.run("prerequest", "1", "r", context(), &mut no_requests).unwrap().skip_request);
        assert!(sandbox.run("prerequest", "pm.execution.skipRequest();", "r", context(), &mut no_requests).unwrap().skip_request);
    }

    #[test]
    fn a_delay_no_clock_holds_runs_out_of_time_instead_of_taking_the_app_down() {
        for delay in ["Infinity", "1e308", "1e22"] {
            let sandbox = Sandbox::boot(Limits { running: Duration::from_secs(10), whole: Duration::from_millis(100) }).unwrap();
            let error = sandbox.run("prerequest", &format!("setTimeout(() => {{}}, {delay});"), "r", context(), &mut no_requests).unwrap_err();
            assert_eq!(error.to_string(), "a script ran out of time", "{delay}");
        }
    }

    #[test]
    fn an_error_in_a_callback_is_said_and_a_rejection_handled_is_not() {
        let code = "pm.sendRequest('https://id.test/', (err, res) => { pm.globals.set('t', res.json().access_token); });";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut |_| Ok(json!({"code": 502, "header": [], "body": "<html>"}))).unwrap();
        assert!(ran.error.is_some(), "the request must not go on with the old token");

        let ran = sandbox().run("prerequest", "Promise.reject(new RangeError('too far'));", "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.error.as_deref(), Some("RangeError: too far"));

        let ran = sandbox().run("prerequest", "Promise.reject('plain');", "r", context(), &mut no_requests).unwrap();
        assert!(ran.error.as_deref().is_some_and(|e| e.contains("plain")), "{:?}", ran.error);

        let code = "Promise.reject(new Error('no')).catch(() => {});\nconst later = Promise.reject(new Error('no'));\nlater.catch(() => {});";
        assert_eq!(sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap().error, None);
    }

    #[test]
    fn a_batch_of_requests_stops_at_the_limit_between_them() {
        let sandbox = Sandbox::boot(Limits { running: Duration::from_secs(10), whole: Duration::from_millis(250) }).unwrap();
        let mut asked = 0;
        let code = "for (let i = 0; i < 8; i++) pm.sendRequest('https://a.test/', () => {});";
        let error = sandbox
            .run("prerequest", code, "r", context(), &mut |_| {
                asked += 1;
                std::thread::sleep(Duration::from_millis(100));
                Ok(json!({"code": 200, "header": [], "body": ""}))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "a script ran out of time");
        assert!(asked < 8, "{asked}");
    }

    #[test]
    fn the_old_ways_of_testing_still_count() {
        let mut context = context();
        context["response"] = json!({"code": 200, "header": [], "body": "{\"id\":42}"});
        let code = "tests['by code'] = responseCode.code === 200;\ntests['by body'] = responseBody.indexOf('id') > -1;\npm.test('ok', () => pm.response.to.be.ok);\npm.test('schema', () => pm.response.to.have.jsonSchema({type: 'object', required: ['id']}));";
        let ran = sandbox().run("test", code, "r", context, &mut no_requests).unwrap();
        assert_eq!(ran.error, None);
        let mut names: Vec<(&str, bool)> = ran.tests.iter().map(|t| (t.name.as_str(), t.passed)).collect();
        names.sort();
        assert_eq!(names, vec![("by body", true), ("by code", true), ("ok", true), ("schema", true)]);
    }

    #[test]
    fn a_library_loaded_as_text_can_write_to_a_window_of_the_scripts_own() {
        let code = "window = {};\nnew Function('window.q = 1')();\npm.environment.set('q', String(window.q));";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.error, None);
        assert!(environment(&ran).contains(&("q".to_string(), "1".to_string())), "{:?}", environment(&ran));
    }

    #[test]
    fn a_heap_past_the_limit_is_refused() {
        let code = "try { pm.environment.set('got', String('x'.repeat(512 * 1024 * 1024).length)); } catch (e) { pm.environment.set('got', 'refused'); }";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert!(environment(&ran).contains(&("got".to_string(), "refused".to_string())), "{:?}", environment(&ran));
    }

    #[test]
    fn requests_one_after_another_stop_at_the_limit_too() {
        let sandbox = Sandbox::boot(Limits { running: Duration::from_secs(10), whole: Duration::from_millis(200) }).unwrap();
        let started = Instant::now();
        let code = "const again = () => pm.sendRequest('https://a.test/', again); again();";
        let error = sandbox
            .run("prerequest", code, "r", context(), &mut |_| {
                assert!(started.elapsed() < Duration::from_secs(20), "still going");
                Ok(json!({"code": 200, "header": [], "body": ""}))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "a script ran out of time");
    }

    #[test]
    fn an_error_in_a_timer_is_said() {
        let ran = sandbox().run("prerequest", "setTimeout(() => { throw new Error('late'); }, 1);", "r", context(), &mut no_requests).unwrap();
        assert!(ran.error.as_deref().is_some_and(|e| e.contains("late")), "{:?}", ran.error);
    }

    #[test]
    fn a_test_skipped_is_neither_passed_nor_failed() {
        let ran = sandbox().run("test", "pm.test.skip('later', () => {});", "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.tests.iter().map(|t| (t.name.as_str(), t.skipped)).collect::<Vec<_>>(), vec![("later", true)]);
    }

    #[test]
    fn a_long_line_is_cut_between_characters_and_a_value_holding_itself_ends() {
        let code = "console.log('a' + 'я'.repeat(3000));\nconst loop = {};\nloop.self = loop;\nconsole.log(loop);";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert!(ran.console[0].text.ends_with("я…"), "{}", ran.console[0].text.len());
        assert!(ran.console[1].text.contains("[…]"), "{}", ran.console[1].text);
    }

    #[test]
    fn a_console_in_a_loop_is_kept_to_its_limit() {
        let code = "for (let i = 0; i < 600; i++) console.log('x'.repeat(5000));";
        let ran = sandbox().run("prerequest", code, "r", context(), &mut no_requests).unwrap();
        assert_eq!(ran.console.len(), CONSOLE_LINES);
        assert_eq!(ran.console[0].text.chars().count(), CONSOLE_LINE + 1);
    }
}
