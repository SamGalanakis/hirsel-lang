use super::*;
use crate::tasks::free::objects::tests::{plain, wrapper};
use crate::tasks::*;
use std::sync::atomic::{AtomicU32, Ordering};

static DROPPED: AtomicU32 = AtomicU32::new(0);
unsafe extern "C" fn leaf(_: *mut TaskCtx, value:*mut u8, _:u64, _:u64)->u8 {
    assert_eq!(unsafe {value.cast::<[u64;2]>().read()}, [1,7]);
    DROPPED.fetch_add(1, Ordering::Relaxed);
    STATUS_OK
}
fn step(kind:u32, offset:u64)->crate::Step {
    crate::Step { kind, offset, field:crate::WHOLE_VALUE, variant:0, span:0,
        stride:0,count:0,count_offset:crate::NO_OFFSET,layout:ptr::null() }
}

#[test]
fn nested_mutex_close_keeps_both_images_cursor_ancestry_and_shields_at_pending_child() {
    for durable in [false,true] {
        DROPPED.store(0,Ordering::Relaxed);
        let scalar=crate::tasks::free::finalized(crate::Layout {finalizer:Some(leaf),..plain(16)});
        let close=Box::leak(Box::new(step(crate::STEP_CLOSE_HANDLE,0)));
        let wrapper=crate::Layout {steps:close,step_count:1,..wrapper()};
        let release=Box::leak(Box::new(step(crate::STEP_RELEASE_ROOT,8)));
        let image=crate::Layout {steps:release,step_count:1,..plain(16)};
        let layouts=[scalar as *const crate::Layout,&wrapper,&image];
        let mut run=crate::host::BodyRun::new(1,false);
        run.config.layouts=layouts.as_ptr();run.config.layout_count=3;run.free=true;
        unsafe {run.use_physical_dispatch()};
        let recording=record::Recording::new(b"nested owned close",durable);
        let install=recording.clone();
        let outcome=run.run(move |ctx| unsafe {crate::tasks::free::enable(ctx);install.root(ctx)}, move |ctx| unsafe {
            let _role=role::Guard::new(ctx);
            let none=[0u64;2];let mut inner=0;let mut payload=[1u64,7];
            hirsel_rt_v2_mutex_create(ctx,ptr::null_mut(),&mut inner,payload.as_mut_ptr().cast(),none.as_ptr().cast(),2,3);
            let mut outer=0;let mut payload=[1u64,inner];
            hirsel_rt_v2_mutex_create(ctx,ptr::null_mut(),&mut outer,payload.as_mut_ptr().cast(),none.as_ptr().cast(),4,3);
            let block=outer as *mut Header;
            let base=(*ctx).work;
            let cursor=Box::new(Cursor::new(base,block,ptr::null(),false,31,41));
            let index=attach(ctx,cursor,31,41);
            assert!(crate::roots::consume(&mut *ctx,block));
            crate::release::push(&mut *ctx,block,true);
            let driver=&mut *(&mut (*ctx).free_ops.tails.drivers)[index] as *mut Driver;
            let inner_word=mutex_word(&inner);
            for _ in 0..3 {
                let result=(*driver).poll_with(ctx, |object,entered| {
                    if !*entered {record::publish(ctx,hirsel_free::phase::Phase::Entered, |_|())?;*entered=true;}
                    Ok(object!=(crate::free::MUTEX,inner_word))
                }).unwrap();
                assert!(result.is_pending());
                assert_eq!((*ctx).shield,2,"both logical close shields survive");
                assert_eq!((*ctx).detached_images.len(),1,"outer image is retained");
                assert_eq!(DROPPED.load(Ordering::Relaxed),0,"no leaf before inner Done");
                assert_eq!((*ctx).free_ops.tails.stack.len(),2);
                assert!(!(*ctx).work.is_null(),"inline Cleanup has a pinned owner");
            }
            let result=(*driver).poll_with(ctx, |_,_|Ok(true)).unwrap();
            assert!(matches!(result,Poll::Ready(STATUS_OK)));
            (*ctx).free_ops.tails.drivers.pop();
            assert_eq!((*ctx).shield,0);
            assert!((*ctx).work.is_null());
            assert_eq!((*ctx).detached_images.len(),0);
            assert!((*ctx).free_ops.tails.stack.is_empty());
            assert_eq!(DROPPED.load(Ordering::Relaxed),1);
            STATUS_OK
        });
        assert_eq!(outcome.ended,Ok(STATUS_OK));
        assert_eq!(outcome.pending,0);
        recording.recorder.stop("returned",None).unwrap();
    }
}

struct Shared {
    outer:u64,
    inner:u64,
    task:AtomicU32,
    polls:AtomicU32,
    ordinal:std::sync::atomic::AtomicU64,
    saved:std::sync::atomic::AtomicBool,
    go:std::sync::atomic::AtomicBool,
}
unsafe extern "C" fn dispatch(ctx:*mut TaskCtx, env:*mut Header, _:u64,_:u64, frame:*mut u8)->u8 {
    unsafe {
        let shared=&**crate::alloc::payload(env).cast::<*const Shared>();
        let result=frame.add(16).cast::<u64>();
        if frame.cast::<u32>().read()==2 {
            return hirsel_rt_v2_task_finish(ctx,result.cast(),[0u64;2].as_ptr().cast(),16,u32::MAX);
        }
        let _role=role::Guard::new(ctx);
        crate::tasks::compact::waited(ctx);
        if frame.cast::<u32>().read()==0 {
            shared.task.store((*ctx).task_id,Ordering::Release);
            let root=shared.outer as *mut Header;
            let cursor=Box::new(Cursor::new((*ctx).work,root,ptr::null(),false,31,41));
            let index=attach(ctx,cursor,31,41);
            frame.add(8).cast::<usize>().write(index);
            assert!(crate::roots::consume(&mut *ctx,root));
            crate::release::push(&mut *ctx,root,true);
            frame.cast::<u32>().write(1);
        }
        let index=frame.add(8).cast::<usize>().read();
        let driver=&mut *(&mut (*ctx).free_ops.tails.drivers)[index] as *mut Driver;
        let polled=(*driver).poll_with(ctx,|object,entered| {
            if !*entered {record::publish(ctx,hirsel_free::phase::Phase::Entered,|_|())?;*entered=true;}
            if object==(crate::free::MUTEX,shared.inner) {
                let ordinal=record::position(ctx).1.op;
                let before=shared.ordinal.compare_exchange(0,ordinal,Ordering::AcqRel,Ordering::Acquire).unwrap_or_else(|old|old);
                assert!(before==0 || before==ordinal,"a wake must keep the child's original ordinal");
                shared.polls.fetch_add(1,Ordering::Release);
                Ok(shared.go.load(Ordering::Acquire))
            } else {Ok(true)}
        }).unwrap();
        if polled.is_pending() {
            assert_eq!((*ctx).shield,2);
            assert_eq!((*ctx).detached_images.len(),1);
            assert_eq!(DROPPED.load(Ordering::Relaxed),0);
            crate::tasks::park_left(ctx,Wait::Retry{cancelable:false}).unwrap();
            crate::tasks::compact::wait(ctx);
            return crate::STATUS_SUSPENDED;
        }
        assert!(matches!(polled,Poll::Ready(STATUS_OK)));
        (*ctx).free_ops.tails.drivers.pop();
        assert_eq!((*ctx).shield,0);
        assert!((*ctx).work.is_null());
        assert_eq!(DROPPED.load(Ordering::Relaxed),1);
        result.write(1);result.add(1).write(108);
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx,result.cast(),[0u64;2].as_ptr().cast(),16,u32::MAX)
    }
}
unsafe extern "C" fn child(ctx:*mut TaskCtx, env:*mut Header, caller:u64,site:u64)->u8 {
    unsafe {crate::continuation::hirsel_rt_v2_resume_run(ctx,env,caller,site,dispatch,32)}
}

#[test]
fn nested_close_pending_child_has_no_stack_and_duplicate_wakes_preserve_effect_and_leaf_count() {
    use crate::tasks::free::objects::tests::{isolated_at,wait_until};
    use std::sync::{Arc,atomic::AtomicBool};
    if !isolated_at("tasks::free::tail::close::tests::nested_close_pending_child_has_no_stack_and_duplicate_wakes_preserve_effect_and_leaf_count","HIRSEL_NESTED_CLOSE_COMPACT") {return;}
    for durable in [false,true] {
        DROPPED.store(0,Ordering::Relaxed);
        let scalar=crate::tasks::free::finalized(crate::Layout {finalizer:Some(leaf),..plain(16)});
        let close=Box::leak(Box::new(step(crate::STEP_CLOSE_HANDLE,0)));
        let wrapper=crate::Layout {steps:close,step_count:1,..wrapper()};
        let release=Box::leak(Box::new(step(crate::STEP_RELEASE_ROOT,8)));
        let image=crate::Layout {steps:release,step_count:1,..plain(16)};
        let env=plain(8);
        let layouts=[scalar as *const crate::Layout,&wrapper,&image,&env];
        let mut run=crate::host::BodyRun::new(4,false);
        run.config.layouts=layouts.as_ptr();run.config.layout_count=4;run.free=true;
        unsafe {run.use_physical_dispatch()};
        let recording=record::Recording::new(b"nested compact close",durable);
        let install=recording.clone();
        let outcome=run.run(move |ctx|unsafe{crate::tasks::free::enable(ctx);install.root(ctx)},move |ctx|unsafe {
            let none=[0u64;2];let mut inner=0;let mut value=[1u64,7];
            hirsel_rt_v2_mutex_create(ctx,ptr::null_mut(),&mut inner,value.as_mut_ptr().cast(),none.as_ptr().cast(),2,3);
            let mut outer=0;let mut value=[1u64,inner];
            hirsel_rt_v2_mutex_create(ctx,ptr::null_mut(),&mut outer,value.as_mut_ptr().cast(),none.as_ptr().cast(),4,3);
            let shared=Arc::new(Shared{outer,inner:mutex_word(&inner),task:AtomicU32::new(u32::MAX),polls:AtomicU32::new(0),ordinal:Default::default(),saved:AtomicBool::new(false),go:AtomicBool::new(false)});
            let hook=shared.clone();
            crate::pool::free::test_after_compact(ctx,move |task,_,pooled| {if task==hook.task.load(Ordering::Acquire) {assert!(pooled);hook.saved.store(true,Ordering::Release);}});
            let mut scope=0;hirsel_rt_v2_task_scope_enter(ctx,ptr::null_mut(),&mut scope,&0);
            let env=crate::alloc::hirsel_rt_v2_alloc(ctx,5,8,8,crate::alloc::DOMAIN_HEAP,0);
            crate::alloc::payload(env).cast::<*const Shared>().write(Arc::as_ptr(&shared));
            let mut task=0;hirsel_rt_v2_task_spawn(ctx,0,0,ptr::null_mut(),&mut task,&scope,child,env,0);
            wait_until("nested close physically saves",||shared.saved.load(Ordering::Acquire));
            let token={let _role=role::Guard::new(ctx);role::with_held(ctx,|s| {
                let task=shared.task.load(Ordering::Acquire);assert!(s.task(task).stack.is_none(),"pending nested close owns no stack");
                crate::pool::free::token((*ctx).run_ptr(),s.key(task),Owner::Any).unwrap()
            })};
            crate::pool::free::notify((*ctx).run_ptr(),&token);crate::pool::free::notify((*ctx).run_ptr(),&token);
            wait_until("spurious wake rechecks same inner close",||shared.polls.load(Ordering::Acquire)>=2);
            assert_eq!(DROPPED.load(Ordering::Relaxed),0);
            shared.go.store(true,Ordering::Release);crate::pool::free::notify((*ctx).run_ptr(),&token);drop(token);
            let mut result=none;hirsel_rt_v2_task_join(ctx,0,0,&scope,&mut task,result.as_mut_ptr().cast(),16);
            assert_eq!(result,[1,108]);hirsel_rt_v2_task_scope_exit(ctx,0,0,&scope,0);
            assert_eq!(DROPPED.load(Ordering::Relaxed),1);
            STATUS_OK
        });
        assert_eq!(outcome.ended,Ok(STATUS_OK));assert_eq!(outcome.pending,0);
        recording.recorder.stop("returned",None).unwrap();
    }
}
