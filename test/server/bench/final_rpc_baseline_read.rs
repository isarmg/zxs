//! Baseline transport: authorize and pread the immutable descriptor.
use super::*;

pub(super) fn read_cycle(client: &Client, index: usize) -> (u128, u128, Vec<u8>) {
    let request = open_request(path_for(index), false);
    let started = Instant::now();
    let (opened, fd) = client.open(request).unwrap();
    let opened_ns = started.elapsed().as_nanos();
    client
        .call(Request::AuthorizeRead {
            handle: opened.handle,
        })
        .unwrap();
    let count = opened.read_count(false, 0, 4096);
    let mut bytes = vec![0; count];
    let file = File::from(fd);
    file.read_exact_at(&mut bytes, 0).unwrap();
    close_descriptor(opened.handle, file.into(), &Mutex::new(()), |request| {
        client.call(request).map(|_| ())
    })
    .unwrap();
    (started.elapsed().as_nanos(), opened_ns, bytes)
}

pub(super) fn directory_paths(pages: Vec<Value>) -> Vec<String> {
    pages
        .into_iter()
        .flat_map(|page| {
            json_codec::from_value::<smb_protocol::protocol::DirectoryPage>(page)
                .unwrap()
                .entries
        })
        .map(|entry| entry.path().to_owned())
        .collect()
}
