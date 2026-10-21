//! Real pinned Windows Node protocol and cancellation tests, using private
//! disposable directories. No external service or production project is used.
use super::*;
use platform_host::{disposable_node_directory, WindowsSourceDirectory};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION},
};

const SCRIPT: &str = r#"const fs=require('fs'),path=require('path'),http=require('http'),cp=require('child_process'),readline=require('readline');
function mark(name,value){const p=path.join(process.env.APP_DATA_DIR,name);fs.writeFileSync(p+'.pending',String(value));fs.renameSync(p+'.pending',p);}
const ordinary=process.env.TRIAL_BEHAVIOR==='ordinary-descendants';
function spawnDescendant(depth){return cp.spawn(process.execPath,[__filename,'descendant',String(depth)],{stdio:ordinary?['pipe','ignore','ignore']:'ignore',windowsHide:true,detached:!ordinary});}
async function stopDescendant(child){if(child){child.stdin.end('shutdown\n');await new Promise(resolve=>child.once('exit',resolve));}}
if(process.argv[2]==='descendant'){
 const depth=Number(process.argv[3]);mark('pid-child-'+depth,process.pid);
 const child=depth>0?spawnDescendant(depth-1):null;
 if(child)child.unref();
 if(ordinary)process.stdin.once('data',async()=>{await stopDescendant(child);process.exit(0);});
 setInterval(()=>{},1000);
}else{
 const mode=process.env.TRIAL_BEHAVIOR;let server,token,instance,descendant;
 const lines=readline.createInterface({input:process.stdin});
 (async()=>{for await(const line of lines){const msg=JSON.parse(line);
 if(msg.type==='initialize'){
  ({token,instanceId:instance}=msg);mark('pid-main',process.pid);
  if(process.env.PATH||process.env.NODE_OPTIONS||process.env.HOME||!process.env.SYSTEMROOT||process.env.TEMP!==process.env.APP_TEMP_DIR||process.env.TMP!==process.env.APP_TEMP_DIR||process.env.TEST_VALUE!=='人工值 空格')throw Error('controlled environment mismatch');
  const count=path.join(process.env.APP_DATA_DIR,'count');mark('count',fs.existsSync(count)?Number(fs.readFileSync(count,'utf8'))+1:1);
  fs.writeFileSync(path.join(process.env.APP_TEMP_DIR,'fresh'),'temporary');
  if(mode==='descendants'||mode==='exit-leader'||ordinary){
   descendant=spawnDescendant(1);descendant.unref();
   while(!fs.existsSync(path.join(process.env.APP_DATA_DIR,'pid-child-0')))await new Promise(r=>setTimeout(r,10));
  }
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'content-length':0});return res.end();}
   const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
   res.writeHead(200,{'content-length':Buffer.byteLength(body)});res.end(body);
   if(req.url==='/exit')setTimeout(()=>process.exit(0),30);
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));mark('port',server.address().port);
  if(mode!=='no-ready')process.stdout.write(JSON.stringify({protocolVersion:1,type:'ready',instanceId:mode==='bad-ready'?'wrong-instance':instance,host:'127.0.0.1',port:server.address().port})+'\n');
 }else if(msg.type==='ping')process.stdout.write(JSON.stringify({protocolVersion:1,type:'pong',instanceId:instance,requestId:msg.requestId})+'\n');
 else if(msg.type==='shutdown'){
  if(mode==='ignore-stop')continue;
  if(ordinary)await stopDescendant(descendant);
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'stopping',instanceId:instance})+'\n');
  await new Promise(r=>server.close(r));lines.close();break;
 }
 }})().catch(()=>process.exit(17));
}
"#;

struct Fixture {
    source: WindowsSourceDirectory,
    root: WindowsSourceDirectory,
    data: WindowsSourceDirectory,
    cache: WindowsSourceDirectory,
    // Retained HANDLE guards must drop before the disposable directory cleanup.
    _temporary: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("Node 中文 空格 '")
            .tempdir()
            .unwrap();
        let root = disposable_node_directory(
            &temporary
                .path()
                .join(format!("windows-node-disposable-{}", Uuid::new_v4())),
        )
        .unwrap();
        let source = root.create_private_directory("source").unwrap();
        source.write_new("managed.cjs", SCRIPT.as_bytes()).unwrap();
        let data = root.create_private_directory("data").unwrap();
        let cache = root.create_private_directory("cache").unwrap();
        Self {
            source,
            root,
            data,
            cache,
            _temporary: temporary,
        }
    }
    fn config(&self, behavior: &str) -> RuntimeLaunchConfig {
        let trial = self
            .root
            .create_private_directory(&format!("node-runtime-trial-{}", Uuid::new_v4()))
            .unwrap();
        RuntimeLaunchConfig {
            node_path: windows_process::tests::private_node(),
            expected_node_sha256:
                "ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c".into(),
            entry_path: self.source.path().join("managed.cjs"),
            working_directory: self.source.path().into(),
            trial_root: trial.path().into(),
            business_env: BTreeMap::from([
                ("TRIAL_BEHAVIOR".into(), behavior.into()),
                ("TEST_VALUE".into(), "人工值 空格".into()),
            ]),
        }
    }
    async fn launch(&self, behavior: &str) -> NodeRuntimeTrial {
        NodeRuntimeTrial::launch_with_storage(
            self.config(behavior),
            self.data.path().into(),
            self.cache.path().into(),
        )
        .await
        .unwrap()
    }
    fn handles(&self, descendants: bool) -> Vec<OwnedHandle> {
        let mut names = vec!["pid-main"];
        if descendants {
            names.extend(["pid-child-1", "pid-child-0"]);
        }
        names
            .into_iter()
            .map(|name| {
                let pid = std::fs::read_to_string(self.data.path().join(name))
                    .unwrap()
                    .parse()
                    .unwrap();
                let handle =
                    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
                assert_ne!(handle, null_mut());
                unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
            })
            .collect()
    }
}
async fn gone(handles: &[OwnedHandle]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handles.iter().any(|handle| unsafe {WaitForSingleObject(handle.as_raw_handle() as HANDLE,0)}!=WAIT_OBJECT_0) {
        assert!(tokio::time::Instant::now()<deadline,"owned fixture process did not exit");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn port_closed(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture listener remained open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn windows_environment_case_aliases_are_rejected_before_process_creation() {
    for key in [
        "Path",
        "path",
        "Node_Options",
        "SYSTEMROOT",
        "SystemRoot",
        "Temp",
        "TMP",
        "APP_PORT",
        "app_port",
    ] {
        assert_eq!(
            validate_business_environment(&BTreeMap::from([(
                key.into(),
                "never-disclose-this-value".into()
            )]))
            .unwrap_err()
            .code,
            "TRIAL_ENV_INVALID"
        );
    }
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn private_node_protocol_normal_shutdown_and_reopen_preserve_persistent_storage() {
    let fixture = Fixture::new();
    let mut first = fixture.launch("").await;
    first.ping().await.unwrap();
    first.verify_health().await.unwrap();
    let handles = fixture.handles(false);
    let old_temp = first.status.run_directory.join("tmp");
    let stopped = first.stop().await.unwrap();
    assert_eq!(stopped.state, "stopped");
    assert!(stopped.stopping_observed && stopped.process_group_exited && !stopped.forced);
    gone(&handles).await;
    port_closed(stopped.port.unwrap()).await;
    drop(first);
    let mut second = fixture.launch("").await;
    assert_ne!(old_temp, second.status.run_directory.join("tmp"));
    assert_eq!(
        std::fs::read_to_string(fixture.data.path().join("count")).unwrap(),
        "2"
    );
    assert_eq!(second.stop().await.unwrap().state, "stopped");
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn protocol_shutdown_reclaims_live_children_and_reports_forced_cleanup_as_failed() {
    let fixture = Fixture::new();
    let mut runtime = fixture.launch("descendants").await;
    let handles = fixture.handles(true);
    let stopped = runtime.stop().await.unwrap();
    assert!(
        stopped.stopping_observed && stopped.forced && stopped.process_group_exited,
        "unexpected detached-tree shutdown: {stopped:?}"
    );
    assert_eq!(stopped.state, "failed");
    gone(&handles).await;
    port_closed(stopped.port.unwrap()).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn ordinary_children_that_finish_their_shutdown_keep_the_runtime_stopped_cleanly() {
    let fixture = Fixture::new();
    let mut runtime = fixture.launch("ordinary-descendants").await;
    let handles = fixture.handles(true);
    let stopped = runtime.stop().await.unwrap();
    assert_eq!(
        stopped.state, "stopped",
        "ordinary-tree shutdown: {stopped:?}"
    );
    assert!(stopped.stopping_observed && stopped.process_group_exited && !stopped.forced);
    gone(&handles).await;
    port_closed(stopped.port.unwrap()).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn unresponsive_shutdown_is_bounded_and_never_reported_as_normal() {
    let fixture = Fixture::new();
    let mut runtime = fixture.launch("ignore-stop").await;
    let handles = fixture.handles(false);
    let stopped = tokio::time::timeout(Duration::from_secs(9), runtime.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(stopped.forced && stopped.process_group_exited);
    assert_eq!(stopped.state, "failed");
    gone(&handles).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn exited_leader_retains_descendant_ownership_and_closes_private_port() {
    let fixture = Fixture::new();
    let mut runtime = fixture.launch("exit-leader").await;
    let handles = fixture.handles(true);
    runtime.get_json("/exit").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(runtime.status().unwrap().state, "failed");
    let stopped = runtime.stop().await.unwrap();
    assert!(stopped.process_group_exited);
    assert_eq!(stopped.state, "failed");
    gone(&handles).await;
    port_closed(stopped.port.unwrap()).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn invalid_ready_and_cancelled_start_never_leave_a_private_listener() {
    let fixture = Fixture::new();
    let error = NodeRuntimeTrial::launch_with_storage(
        fixture.config("bad-ready"),
        fixture.data.path().into(),
        fixture.cache.path().into(),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.code, "TRIAL_READY_INVALID");
    let pid = std::fs::read_to_string(fixture.data.path().join("pid-main"))
        .unwrap()
        .parse()
        .unwrap();
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
    } else {
        gone(&[unsafe { OwnedHandle::from_raw_handle(handle.cast()) }]).await;
    }
    let port = std::fs::read_to_string(fixture.data.path().join("port"))
        .unwrap()
        .parse()
        .unwrap();
    port_closed(port).await;
    std::fs::remove_file(fixture.data.path().join("port")).unwrap();
    let config = fixture.config("no-ready");
    let data = fixture.data.path().to_owned();
    let cache = fixture.cache.path().to_owned();
    let pending =
        tokio::spawn(
            async move { NodeRuntimeTrial::launch_with_storage(config, data, cache).await },
        );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if fixture.data.path().join("port").exists() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let handles = fixture.handles(false);
    pending.abort();
    assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
    gone(&handles).await;
    let port = std::fs::read_to_string(fixture.data.path().join("port"))
        .unwrap()
        .parse()
        .unwrap();
    port_closed(port).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn invalid_digest_is_rejected_before_any_application_code_runs() {
    let fixture = Fixture::new();
    let mut config = fixture.config("");
    config.expected_node_sha256 = "0".repeat(64);
    assert_eq!(
        NodeRuntimeTrial::launch(config).await.err().unwrap().code,
        "TRIAL_NODE_DIGEST_MISMATCH"
    );
    assert!(!fixture.data.path().join("pid-main").exists());
}
