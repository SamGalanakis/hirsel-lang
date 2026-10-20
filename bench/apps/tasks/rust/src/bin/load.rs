//! Repository-pinned open-loop HTTP/SSE driver. Latencies start at scheduled
//! arrival time, so time spent waiting for a connection is included.
use futures_util::{StreamExt, stream::FuturesUnordered};
use hdrhistogram::Histogram;
use serde_json::json;
use std::{env, io::{self, BufRead, Write}, time::{Duration, Instant}};

fn report(hist: &Histogram<u64>, elapsed: f64, completed: u64, errors: u64) {
    println!("{}",json!({"completed":completed,"errors":errors,"elapsed_s":elapsed,"requests_per_s":completed as f64/elapsed,
        "p50_us":hist.value_at_quantile(0.5),"p99_us":hist.value_at_quantile(0.99),"p999_us":hist.value_at_quantile(0.999),"max_us":hist.max()}));
}
async fn event<S,E>(stream: &mut S, pending: &mut Vec<u8>) -> Result<(),String>
where S: futures_util::Stream<Item=Result<bytes::Bytes,E>> + Unpin, E: std::fmt::Display {
    loop {
        if let Some(i)=pending.windows(2).position(|w|w==b"\n\n") {
            let block:Vec<_>=pending.drain(..i+2).collect();
            if !block.windows(b"datastar-patch-elements".len()).any(|w|w==b"datastar-patch-elements") {return Err("unexpected SSE event".into());}
            return Ok(());
        }
        match stream.next().await { Some(Ok(bytes))=>pending.extend_from_slice(&bytes),Some(Err(e))=>return Err(e.to_string()),None=>return Err("SSE ended".into()) }
    }
}
#[tokio::main(flavor="current_thread")]
async fn main() -> Result<(),Box<dyn std::error::Error>> {
    let args:Vec<_>=env::args().collect();
    let port=&args[1]; let op=&args[2];let rate:u64=args[3].parse()?;let duration:f64=args[4].parse()?;
    let base=format!("http://127.0.0.1:{port}");
    let client=reqwest::Client::builder().pool_max_idle_per_host(256).timeout(Duration::from_secs(60)).build()?;
    // For SSE, open every stream and consume its initial patch before enabling perf.
    let mut streams=Vec::new();
    if op=="sse" {
        let screen=r#"{"filter":{"status":"","tag":"","text":"Task 9999","sort":"manual"},"selected":0}"#;
        for _ in 0..rate {
            let response=client.get(format!("{base}/updates")).query(&[("datastar",screen)]).send().await?.error_for_status()?;
            let mut stream=response.bytes_stream();let mut pending=Vec::new();
            tokio::time::timeout(Duration::from_secs(60),event(&mut stream,&mut pending)).await??;
            streams.push((stream,pending));
        }
    }
    println!("ready");io::stdout().flush()?;
    let mut command=String::new();io::stdin().lock().read_line(&mut command)?;
    if command.trim()!="go" {return Err("expected go".into());}
    let start=Instant::now();let mut hist=Histogram::<u64>::new(3)?;
    if op=="sse" {
        let response=client.post(format!("{base}/tasks/1/advance")).send().await?;
        if response.status().as_u16()!=204 {return Err(format!("mutation: {}",response.status()).into());}
        let mut receiving=FuturesUnordered::new();
        for (mut stream,mut pending) in streams {receiving.push(async move {
            tokio::time::timeout(Duration::from_secs(60),event(&mut stream,&mut pending)).await.map_err(|e|e.to_string())??;
            Ok::<_,String>(start.elapsed().as_micros() as u64)
        });}
        while let Some(latency)=receiving.next().await {hist.record(latency?.max(1))?;}
        report(&hist,start.elapsed().as_secs_f64(),rate,0);return Ok(());
    }
    let total=(rate as f64*duration).ceil() as u64;
    let mut outstanding=FuturesUnordered::new();let mut errors=0;let mut completed=0;
    // Dispatch from one producer independently of completions. Bounding the sample
    // by 4096 outstanding requests refuses overload; never silently drops arrivals.
    for n in 0..total {
        let scheduled=start+Duration::from_secs_f64(n as f64/rate as f64);
        tokio::time::sleep_until(scheduled.into()).await;
        let request=match op.as_str() {
            "list"=>client.get(format!("{base}/")),
            "search"=>client.get(format!("{base}/?q=Task+9999&sort=priority")),
            "create"=>client.post(format!("{base}/tasks")).json(&json!({"draft":{"title":"Benchmark task","description":"benchmark","priority":"medium","due":"","tags":"bench"}})),
            "update"=>client.post(format!("{base}/tasks/1")).header("content-type","application/x-www-form-urlencoded").body("title=Updated&description=benchmark&priority=high&due=&tags=bench"),
            _=>return Err("unknown operation".into())
        };
        outstanding.push(tokio::spawn(async move {
            let response=request.send().await?;let status=response.status();let bytes=response.bytes().await?;
            Ok::<_,reqwest::Error>((status.is_success() && !bytes.is_empty(),scheduled.elapsed().as_micros() as u64))
        }));
        // Drain only ready completions; the arrival clock never waits for a response.
        while let Some(Some(result))=outstanding.next().now_or_never() {
            match result {Ok(Ok((true,latency)))=>{hist.record(latency.max(1))?;completed+=1;},_=>errors+=1}
        }
        if outstanding.len()>4096 {return Err("overload: 4096 outstanding arrivals".into());}
    }
    while let Some(result)=outstanding.next().await {
        match result {Ok(Ok((true,latency)))=>{hist.record(latency.max(1))?;completed+=1;},_=>errors+=1}
    }
    report(&hist,start.elapsed().as_secs_f64(),completed,errors);
    if errors!=0 {return Err(format!("{errors} failed requests").into());} Ok(())
}
use futures_util::FutureExt;
