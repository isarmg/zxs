use super::*;

#[test]
fn logical_units_keep_real_free_distinct_from_caller_available_even_with_read_only_policy(){
    let mut sample=Capacity{total:4096*1024+17,available:4096*12+4095,free:4096*49+31,read_only:false};
    let rw=CapacityUnits{total:1024,caller_available:12,actual_free:49};
    let ro=CapacityUnits{caller_available:0,..rw};
    assert_eq!(CapacityUnits::from_sample(&sample,true),rw);
    assert_eq!(CapacityUnits::from_sample(&sample,false),ro);
    sample.read_only=true;
    assert_eq!(CapacityUnits::from_sample(&sample,false),ro);
    assert_eq!(CapacityUnits::from_sample(&sample,true),ro);
}
