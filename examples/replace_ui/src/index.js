import './style';
import { Component } from 'preact';

const COUNTER_URL = "http://localhost:8080/counter1"

export default class App extends Component {
	constructor(props) {
		super(props)

		this.state = {
			payload: null,
			error: "",
		}
	}

	render() {
		return (
			<div className="main">
				<img src="https://miro.medium.com/max/400/1*-wSVG5Qyg80Fu2bt6Tvs2w.png" style={{ height: "350px", marginTop: "-25px", marginBottom: "-50px" }} onClick={this.counterRequest}></img>
				<h1>waSCC Counter Demo</h1>
				<div className="request">
					<a onClick={this.counterRequest} data-title="Count!"></a>
					{this.state.payload ?
						<div className="payload">
							{JSON.stringify(this.state.payload).replace(",", ",\n  ").replace("{", "{\n  ").replace("}", "\n}")}
						</div>
						:
						<div className="payload" style={{ color: 'white' }}>
							secret
							<br />
						</div>
					}
				</div>
				<br />
				<h2>No boilerplate!</h2>
				<img src="https://i.imgur.com/w2QWiQL.png"></img>
			</div>
		);
	}

	counterRequest = () => {
		fetch(COUNTER_URL).then(d => d.json()).then(r => this.setState({ counter: r.counter, tweaked: r.tweaked, payloadRequested: true, payload: r })).catch(e => this.setState({ payload: { "error": "error making request" } }))
	}

}