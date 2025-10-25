mod crd;
use crd::ModelDeployment;
use kube::CustomResourceExt;

fn main() {
    let crd = ModelDeployment::crd();
    println!("{}", serde_yaml::to_string(&crd).unwrap());
}
